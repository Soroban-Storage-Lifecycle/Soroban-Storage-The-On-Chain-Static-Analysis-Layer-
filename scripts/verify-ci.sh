#!/usr/bin/env bash
# Mirrors the steps of `.github/actions/soroban-storage-check/action.yml` so the
# CI path can be exercised locally without GitHub. Exits non-zero on failure.
set -euo pipefail

RUST_VERSION="${RUST_VERSION:-1.98.1}"
CONTRACT_PACKAGE="${CONTRACT_PACKAGE:-example-contract}"

echo "==> toolchain: $RUST_VERSION + wasm32v1-none + clippy/rustfmt"
rustup toolchain install "$RUST_VERSION" --profile minimal --component clippy,rustfmt
rustup target add --toolchain "$RUST_VERSION" wasm32v1-none
rustup default "$RUST_VERSION"

echo "==> obtain linter (workspace mode)"
cargo build --release -p soroban-storage-lints
export PATH="$PWD/target/release:$PATH"

echo "==> storage lints: example contract (--deny-warnings)"
cargo soroban-lint crates/example-contract/src --deny-warnings

echo "==> storage lints: framework sources (self-check)"
cargo soroban-lint crates/example-contract/src crates/soroban-storage/src --deny-warnings

echo "==> build contract wasm"
cargo build --release --target wasm32v1-none -p "$CONTRACT_PACKAGE"

echo "==> full test suite"
cargo test --workspace

echo "==> build and smoke-test the off-chain tools"
cargo build -p soroban-rent-keeper -p soroban-restore-planner
./target/debug/soroban-rent-keeper --help > /dev/null
./target/debug/soroban-restore-planner --help > /dev/null

set +e
./target/debug/soroban-rent-keeper --bogus > /dev/null 2>&1
keeper_usage=$?
./target/debug/soroban-restore-planner --bogus > /dev/null 2>&1
planner_usage=$?
./target/debug/soroban-restore-planner \
  --contract not-a-strkey --rpc-url http://localhost \
  --network-passphrase Test > /dev/null 2>&1
planner_bad_contract=$?
set -e

[ "$keeper_usage" -eq 2 ] || { echo "keeper usage exit $keeper_usage != 2"; exit 1; }
[ "$planner_usage" -eq 2 ] || { echo "planner usage exit $planner_usage != 2"; exit 1; }
[ "$planner_bad_contract" -eq 1 ] || { echo "planner bad-contract exit $planner_bad_contract != 1"; exit 1; }
echo "off-chain CLI smoke tests OK"

echo "==> clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "CI simulation: OK"