#!/usr/bin/env bash
#
# Deploys the full HoS treasury topology to mainnet:
#
#   Policy DAO ----------> Policy Timelock ----- admin of every timelock ----+
#                          (policy-tl.<parent>)  (its own included), of both |
#                                                spending accounts, and sole |
#                                                policy-changer of all DAOs  |
#                                                                            v
#   Execution DAO -------> Execution Timelock SWF ---- spender ----> SWF (swf.<parent>)
#                          (exec-swf-tl.<parent>)                        | whitelisted
#                                                                        v
#   Payment DAO ---------> Execution Timelock SSA ---- spender ----> SSA (ssa.<parent>)
#                          (exec-ssa-tl.<parent>)
#
# Modes (identical except where the DAOs come from):
#   staging     The three DAOs are deployed by this script from
#               res/sputnikdao2.wasm (v2.3.1) onto *-dao.<parent> subaccounts,
#               with a 1-of-1 council (the parent).
#   production  The DAOs are NOT created by this script: they must already
#               exist (created by their communities, e.g. via the SputnikDAO
#               factory) and be passed in as POLICY_DAO / EXEC_DAO /
#               PAYMENT_DAO. Their policies MUST grant the "policy:*"
#               permissions to the Policy Timelock and to no one else, and
#               the parent must be able to add and approve proposals on the
#               Policy DAO for the whitelisting bootstrap below to work.
#
# In both modes every created account keeps a full-access key, so the whole
# deployment can be deleted and its NEAR recovered, and the SSA is
# whitelisted in the SWF through the real governance path
# (proposal -> vote -> timelock delay -> execute).
#
# Usage:
#   scripts/deploy-mainnet-test.sh staging <parent-account.near>
#   POLICY_DAO=... EXEC_DAO=... PAYMENT_DAO=... \
#     scripts/deploy-mainnet-test.sh production <parent-account.near>
#
# Production deviations handled OUTSIDE this script:
#   - real guardians (set GUARDIANS to a JSON array of account ids);
#   - a real delay (set DELAY_NS);
#   - removing the full-access keys from the contract accounts once verified.
#
# Requires near-cli-rs with the parent's full-access key in the keychain.

set -euo pipefail

usage="Usage: $0 <staging|production> <parent-account.near>"
MODE="${1:?$usage}"
PARENT="${2:?$usage}"
[[ "$MODE" == "staging" || "$MODE" == "production" ]] || { echo "$usage" >&2; exit 1; }

# ---------------------------------------------------------------- config ----
NETWORK="${NETWORK:-mainnet}"

DELAY_NS="${DELAY_NS:-60000000000}"          # 60s timelock delay
DELAY_WAIT_SECONDS="${DELAY_WAIT_SECONDS:-75}"
GUARDIANS="${GUARDIANS:-[\"$PARENT\"]}"      # JSON array of guardian account ids

DAO_BALANCE="${DAO_BALANCE:-5 NEAR}"         # staging: ~434K wasm => ~4.4 NEAR storage
TIMELOCK_BALANCE="${TIMELOCK_BALANCE:-2.5 NEAR}"   # ~202K release wasm => ~2.1 NEAR storage
SPENDING_BALANCE="${SPENDING_BALANCE:-2 NEAR}"     # ~157K release wasm => ~1.6 NEAR storage
SWF_FUNDING="${SWF_FUNDING:-2 NEAR}"         # treasury funds on top of storage

PROPOSAL_BOND="0.1 NEAR"
PROPOSAL_BOND_YOCTO="100000000000000000000000"
SSA_LIMIT_YOCTO="${SSA_LIMIT_YOCTO:-1000000000000000000000000}"  # 1 NEAR

# optional: also whitelist a payout recipient in the SSA.
RECIPIENT="${RECIPIENT:-}"
RECIPIENT_LIMIT_YOCTO="${RECIPIENT_LIMIT_YOCTO:-500000000000000000000000}"  # 0.5 NEAR

# sign-with-keychain falls back to the legacy keychain (~/.near-credentials);
# switch to sign-with-legacy-keychain if your setup needs it.
SIGN_WITH="${SIGN_WITH:-sign-with-keychain}"

# Only reproducible release artifacts (see build_release.sh) are deployed, so
# the contracts can be verified against the source (NEP-330 + SourceScan).
# Build them with build_release.sh before running this script.
TIMELOCK_WASM="res/release/dao_timelock.wasm"
SPENDING_WASM="res/release/spending_account.wasm"
SPUTNIK_WASM="res/sputnikdao2.wasm"

# ---------------------------------------------------------------- naming ----
SWF="swf.$PARENT"
SSA="ssa.$PARENT"
POLICY_TL="policy-tl.$PARENT"
EXEC_SWF_TL="exec-swf-tl.$PARENT"
EXEC_SSA_TL="exec-ssa-tl.$PARENT"
if [[ "$MODE" == "staging" ]]; then
  POLICY_DAO="policy-dao.$PARENT"
  EXEC_DAO="exec-dao.$PARENT"
  PAYMENT_DAO="payment-dao.$PARENT"
else
  dao_usage="production mode requires existing DAO accounts: POLICY_DAO=... EXEC_DAO=... PAYMENT_DAO=... $0 production $PARENT"
  POLICY_DAO="${POLICY_DAO:?$dao_usage}"
  EXEC_DAO="${EXEC_DAO:?$dao_usage}"
  PAYMENT_DAO="${PAYMENT_DAO:?$dao_usage}"
fi

# --------------------------------------------------------------- helpers ----
say() { printf '\n\033[1;34m== %s\033[0m\n' "$*"; }

b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

delay_human() { # renders DELAY_NS as "Xd Xh Xm Xs"
  local s=$((DELAY_NS / 1000000000))
  printf '%dd %dh %dm %ds' $((s / 86400)) $((s % 86400 / 3600)) $((s % 3600 / 60)) $((s % 60))
}

dao_roles() { # <dao> — prints the role names and member groups of the DAO policy
  view "$1" get_policy '{}' | tr -d '\n ' \
    | grep -o '"name":"[^"]*"\|"Group":\[[^]]*\]' || true
}

view() { # <contract> <method> <json-args>
  near contract call-function as-read-only "$1" "$2" json-args "$3" \
    network-config "$NETWORK" now
}

# Views print decorations to stderr and the value to stdout; a bare number or
# string result is the last stdout line.
view_scalar() {
  view "$@" | tail -n 1 | tr -d '[:space:]"'
}

call() { # <contract> <method> <json-args> <deposit> <gas>
  near contract call-function as-transaction "$1" "$2" json-args "$3" \
    prepaid-gas "$5" attached-deposit "$4" \
    sign-as "$PARENT" network-config "$NETWORK" "$SIGN_WITH" send
}

create_subaccount() { # <account-id> <balance>
  near account create-account fund-myself "$1" "$2" \
    autogenerate-new-keypair save-to-legacy-keychain \
    sign-as "$PARENT" network-config "$NETWORK" "$SIGN_WITH" send
}

deploy_contract() { # <account-id> <wasm-path> <init-json-args>
  near contract deploy "$1" use-file "$2" \
    with-init-call new json-args "$3" \
    prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' \
    network-config "$NETWORK" "$SIGN_WITH" send
}

# The staging DAO policy, with a 1-of-1 council: the parent can add and vote
# FunctionCall proposals, and — staging convenience — change the DAO policies
# directly, with no timelock delay. The Policy Timelock also holds the
# "policy:*" permissions so the production governance path can be exercised:
# the Policy DAO changes its own policy via schedule_proposal and the other
# DAOs' via scheduled add_proposal + act_proposal calls. In production only
# the Policy Timelock may hold these permissions (see the checklist).
dao_policy() {
  cat <<EOF
{
  "roles": [
    {
      "name": "council",
      "kind": { "Group": ["$PARENT"] },
      "permissions": [
        "call:AddProposal", "call:VoteApprove", "call:VoteReject", "call:VoteRemove",
        "policy:AddProposal", "policy:VoteApprove", "policy:VoteReject", "policy:VoteRemove"
      ],
      "vote_policy": {}
    },
    {
      "name": "timelock",
      "kind": { "Group": ["$POLICY_TL"] },
      "permissions": ["policy:AddProposal", "policy:VoteApprove"],
      "vote_policy": {}
    }
  ],
  "default_vote_policy": { "weight_kind": "RoleWeight", "quorum": "0", "threshold": [1, 2] },
  "proposal_bond": "$PROPOSAL_BOND_YOCTO",
  "proposal_period": "604800000000000",
  "bounty_bond": "$PROPOSAL_BOND_YOCTO",
  "bounty_forgiveness_period": "86400000000000"
}
EOF
}

# staging only: deploys res/sputnikdao2.wasm onto a keyed subaccount.
create_dao() { # <dao-account-id>
  create_subaccount "$1" "$DAO_BALANCE"
  deploy_contract "$1" "$SPUTNIK_WASM" \
    "{\"config\":{\"name\":\"${1%%.*}\",\"purpose\":\"HoS treasury\",\"metadata\":\"\"},\"policy\":$(dao_policy | tr -d '\n ')}"
}

# The canonical governance path: the council (the parent) proposes a
# FunctionCall on the DAO that asks the timelock to schedule a call on the
# target contract, approves it (1-of-1, so the vote executes the proposal and
# schedules the request), waits out the delay and executes the request.
dao_action() { # <dao> <timelock> <target> <method> <json-args> <deposit-yocto> <tgas>
  local dao="$1" timelock="$2" target="$3" method="$4" args="$5" deposit="$6" tgas="$7"
  local schedule_args kind proposal_id request_id

  schedule_args=$(b64 "{\"receiver_id\":\"$target\",\"actions\":[{\"method_name\":\"$method\",\"args\":\"$(b64 "$args")\",\"deposit\":\"$deposit\",\"gas\":${tgas}000000000000}]}")
  kind="{\"FunctionCall\":{\"receiver_id\":\"$timelock\",\"actions\":[{\"method_name\":\"schedule\",\"args\":\"$schedule_args\",\"deposit\":\"$deposit\",\"gas\":50000000000000}]}}"

  proposal_id=$(view_scalar "$dao" get_last_proposal_id '{}')
  request_id=$(view_scalar "$timelock" get_next_request_id '{}')

  say "Proposal $proposal_id on $dao: $target.$method via $timelock (request $request_id)"
  call "$dao" add_proposal \
    "{\"proposal\":{\"description\":\"$method on $target\",\"kind\":$kind}}" \
    "$PROPOSAL_BOND" '100.0 Tgas'

  # The "proposal" field is required by sputnikdao v2.3.1+ and ignored by
  # older versions, so it is safe to always send it.
  call "$dao" act_proposal \
    "{\"id\":$proposal_id,\"action\":\"VoteApprove\",\"proposal\":$kind}" \
    '0 NEAR' '300.0 Tgas'

  say "Waiting ${DELAY_WAIT_SECONDS}s for the timelock delay..."
  sleep "$DELAY_WAIT_SECONDS"

  call "$timelock" execute "{\"request_id\":$request_id}" '0 NEAR' '300.0 Tgas'
}

# ------------------------------------------------------------- pre-flight ---
cd "$(dirname "$0")/.."
command -v near >/dev/null || { echo "near-cli-rs is not installed." >&2; exit 1; }

# A deployment is only verifiable if the deployed wasm can be rebuilt from a
# commit that is public on the repository named in Cargo.toml.
git merge-base --is-ancestor HEAD "@{upstream}" 2>/dev/null ||
  echo "WARNING: HEAD is not pushed to the upstream branch; push it or source verification will fail." >&2

[[ -f "$TIMELOCK_WASM" && -f "$SPENDING_WASM" ]] || {
  echo "Missing release wasm artifacts in res/release/ — run build_release.sh first." >&2
  exit 1
}

cat <<EOF
Deploying HoS treasury topology as $PARENT on $NETWORK ($MODE mode):
  Policy DAO:              $POLICY_DAO
  Execution DAO:           $EXEC_DAO
  Payment DAO:             $PAYMENT_DAO
  Policy Timelock:         $POLICY_TL
  Execution Timelock SWF:  $EXEC_SWF_TL
  Execution Timelock SSA:  $EXEC_SSA_TL
  SWF (spending-account):  $SWF
  SSA (spending-account):  $SSA
  Timelock admin:          $POLICY_TL
  Guardians:               $GUARDIANS
  Timelock delay:          $DELAY_NS ns ($(delay_human))
EOF
if [[ "$MODE" == "staging" ]]; then
  cat <<EOF
  DAO roles (identical in all three DAOs created by this script):
    council:  $PARENT (1-of-1, call:* and policy:* permissions)
    timelock: $POLICY_TL (policy:AddProposal + policy:VoteApprove)
EOF
else
  # The DAOs already exist: show who actually controls them before deploying.
  echo "Existing DAO policies (roles and members):"
  for dao in "$POLICY_DAO" "$EXEC_DAO" "$PAYMENT_DAO"; do
    echo "  $dao:"
    dao_roles "$dao" | sed 's/^/    /'
  done
fi
read -r -p "Continue? [y/N] " reply
[[ "$reply" == "y" || "$reply" == "Y" ]] || exit 1

# ------------------------------------------------------------ DAOs (staging)
# In production the DAO accounts are taken as-is and only used as the dao_id
# of each timelock.
if [[ "$MODE" == "staging" ]]; then
  say "Deploying the three DAOs (sputnikdao2 v2.3.1, $DAO_BALANCE each, recoverable)"
  create_dao "$POLICY_DAO"
  create_dao "$EXEC_DAO"
  create_dao "$PAYMENT_DAO"
fi

# ------------------------------------------------------------- timelocks ----
# The Policy Timelock is the admin of every timelock, its own included: config
# changes (set_dao, set_admin, set_guardians, set_delay) must be scheduled on
# the Policy Timelock by the Policy DAO and wait out its delay.
say "Deploying the three dao-timelocks (admin: $POLICY_TL, guardians: $GUARDIANS, delay: ${DELAY_NS}ns)"
for pair in "$POLICY_TL:$POLICY_DAO" "$EXEC_SWF_TL:$EXEC_DAO" "$EXEC_SSA_TL:$PAYMENT_DAO"; do
  timelock="${pair%%:*}"
  dao="${pair#*:}"
  create_subaccount "$timelock" "$TIMELOCK_BALANCE"
  deploy_contract "$timelock" "$TIMELOCK_WASM" \
    "{\"dao_id\":\"$dao\",\"admin_id\":\"$POLICY_TL\",\"guardians\":$GUARDIANS,\"delay_ns\":\"$DELAY_NS\"}"
done

# ------------------------------------------------------ spending accounts ---
say "Deploying the SWF and SSA spending accounts"
create_subaccount "$SWF" "$SPENDING_BALANCE"
deploy_contract "$SWF" "$SPENDING_WASM" \
  "{\"admin_id\":\"$POLICY_TL\",\"spender_id\":\"$EXEC_SWF_TL\"}"
create_subaccount "$SSA" "$SPENDING_BALANCE"
deploy_contract "$SSA" "$SPENDING_WASM" \
  "{\"admin_id\":\"$POLICY_TL\",\"spender_id\":\"$EXEC_SSA_TL\"}"

say "Funding the SWF treasury with $SWF_FUNDING"
near tokens "$PARENT" send-near "$SWF" "$SWF_FUNDING" \
  network-config "$NETWORK" "$SIGN_WITH" send

# -------------------------------------------------------------- bootstrap ---
# The parent drives the governance path itself, so it must be able to add and
# approve proposals on the Policy DAO (in staging it is the 1-of-1 council).
say "Whitelisting the SSA in the SWF through the Policy DAO + Policy Timelock"
dao_action "$POLICY_DAO" "$POLICY_TL" "$SWF" add_to_whitelist \
  "{\"account_id\":\"$SSA\",\"token_id\":null,\"limit\":\"$SSA_LIMIT_YOCTO\"}" 0 30

if [[ -n "$RECIPIENT" ]]; then
  say "Whitelisting $RECIPIENT in the SSA through the Policy DAO + Policy Timelock"
  dao_action "$POLICY_DAO" "$POLICY_TL" "$SSA" add_to_whitelist \
    "{\"account_id\":\"$RECIPIENT\",\"token_id\":null,\"limit\":\"$RECIPIENT_LIMIT_YOCTO\"}" 0 30
fi

# ------------------------------------------------------------ verification --
say "Verifying the deployed configuration"
for sa in "$SWF" "$SSA"; do
  echo "$sa admin:   $(view_scalar "$sa" get_admin '{}')"
  echo "$sa spender: $(view_scalar "$sa" get_spender '{}')"
done
for timelock in "$POLICY_TL" "$EXEC_SWF_TL" "$EXEC_SSA_TL"; do
  echo "$timelock dao:   $(view_scalar "$timelock" get_dao '{}')"
  echo "$timelock admin: $(view_scalar "$timelock" get_admin '{}')"
done
# The "timelock" role of every DAO policy must contain only the Policy
# Timelock: it is the sole account allowed to change the DAO policies.
for dao in "$POLICY_DAO" "$EXEC_DAO" "$PAYMENT_DAO"; do
  echo "$dao policy roles:"
  dao_roles "$dao"
done
view "$SWF" get_whitelist_entries '{}'

# NEP-330 metadata embedded by the reproducible build: repository + commit
# that SourceScan uses to verify the deployed code.
say "Embedded source metadata (NEP-330)"
view "$SWF" contract_source_metadata '{}'
view "$POLICY_TL" contract_source_metadata '{}'

say "Done"
cat <<EOF
Smoke test — move funds SWF -> SSA through the Execution DAO:
  1. As $PARENT, add a FunctionCall proposal on $EXEC_DAO calling
     $EXEC_SWF_TL.schedule with a transfer action on $SWF
     (receiver_id: $SSA), vote it through, wait 60s, execute.
Cleanup:
  near account delete-account <subaccount> beneficiary $PARENT \\
    network-config $NETWORK $SIGN_WITH send
Hardening (production):
  1. Verify the policy of every DAO ($POLICY_DAO, $EXEC_DAO, $PAYMENT_DAO):
     the "policy:*" permissions must be granted to $POLICY_TL and to no other
     role, so that only the Policy DAO (through the Policy Timelock and its
     delay) can change any DAO policy.
  2. Fund the SWF with the real treasury balance.
  3. After verifying everything, remove the full-access keys from
     $SWF, $SSA, $POLICY_TL, $EXEC_SWF_TL, $EXEC_SSA_TL:
       near account delete-keys <account> public-keys <pk> \\
         network-config $NETWORK $SIGN_WITH send
EOF
