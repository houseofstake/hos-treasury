#!/bin/bash
set -e

pushd "$(dirname "$0")"

mkdir -p res/local

pushd dao-timelock
cargo near build non-reproducible-wasm
popd
cp target/near/dao_timelock/dao_timelock.wasm res/local/

pushd spending-account
cargo near build non-reproducible-wasm
popd
cp target/near/spending_account/spending_account.wasm res/local/

popd
