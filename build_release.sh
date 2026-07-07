#!/bin/bash
set -e

cd $(dirname $0)
mkdir -p res/release

pushd dao-timelock
cargo near build reproducible-wasm
popd

pushd spending-account
cargo near build reproducible-wasm
popd

cp target/near/dao_timelock/dao_timelock.wasm res/release/
cp target/near/spending_account/spending_account.wasm res/release/
