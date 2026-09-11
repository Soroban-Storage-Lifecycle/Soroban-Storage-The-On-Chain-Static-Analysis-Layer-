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

echo "==> clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "CI simulation: OK"