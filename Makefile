.PHONY: check test lint lint-all wasm fmt clippy clean

# Type-check the whole workspace without building wasm.
check:
	cargo check --workspace

# Run the full host test suite (framework + example contract).
test:
	cargo test --workspace

# Lint a contract's source (default: example contract).
lint:
	cargo run -q -p soroban-storage-lints -- --deny-warnings crates/example-contract/src

# Self-check: lint everything this repository ships.
lint-all:
	cargo run -q -p soroban-storage-lints -- --deny-warnings \
		crates/example-contract/src \
		crates/soroban-storage/src \
		crates/soroban-storage-lints/src

# Build the example contract for the Soroban wasm target.
wasm:
	cargo build --release --target wasm32v1-none -p example-contract

# Install the linter as the `cargo soroban-lint` subcommand.
install-lint:
	cargo install --path crates/soroban-storage-lints

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

clean:
	cargo clean