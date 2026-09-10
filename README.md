# Soroban Storage — The On-Chain & Static Analysis Layer

> Typed storage wrappers that make the *correct* Soroban TTL pattern the
> default, a static analyzer that flags storage misuse at build time, and CI
> wiring that fails builds on violations.

**The problem:** every persistent entry in a Soroban contract has a time-to-live
and needs rent paid or it gets evicted. Get the storage type wrong and you lose
user funds: storing long-term critical data such as user balances in
**temporary** storage causes *irreversible* state loss when the entry's TTL
reaches zero — unlike persistent and instance entries, expired temporary entries
are permanently deleted and **cannot be restored**. Restored persistent entries
only get extended to the network minimum (currently `ledger + 4095`). Both open
SCF RFPs list TTL management as a hazard implementers *must* address, and today
every team writes its own `extend_ttl` cron script.

This repository is the Anchor-style safety layer for Stellar storage — the
safe-defaults library plus the static analyzer that the ecosystem is missing.

## What's inside

| Component | What it does |
|---|---|
| [`crates/soroban-storage`](crates/soroban-storage) | The `storage!` macro + runtime: every key is bound to **exactly one lifecycle** at compile time, accessors **auto-bump TTLs** on read/write, and `critical` entries (balances, funds) are **rejected** in `temporary` storage with a hard compile error. |
| [`crates/soroban-storage-lints`](crates/soroban-storage-lints) | `cargo soroban-lint`: static analysis that flags critical types written through `env.storage().temporary()`, keys accessed through the wrong lifecycle, write paths with no TTL extension, and instance-storage bloat. |
| [`.github/workflows/storage-lint.yml`](.github/workflows/storage-lint.yml) + [packaged action](.github/actions/soroban-storage-check/action.yml) | CI that fails developer builds when storage lint errors are found, and builds the contract for the Soroban wasm target. |
| [`crates/example-contract`](crates/example-contract) | A working vault contract (balances, grants, nonces, admin) demonstrating every feature, with host tests. |

## Quick start

Add the dependency and declare your storage schema:

```toml
[dependencies]
soroban-sdk = "27.0.6"
soroban-storage = { git = "https://github.com/stellar/soroban-storage" }
```

```rust
use soroban_sdk::{contract, contractimpl, Address, Env};
use soroban_storage::{storage, TtlPolicy};

storage! {
    /// User token balance. Irreplaceable — never temporary.
    #[storage(persistent, critical)]
    #[storage(policy = TtlPolicy::days(7, 30))]
    Balance(Address) -> i128,

    #[storage(temporary)]
    Nonce(Address) -> u64,
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    pub fn deposit(env: Env, user: Address, amount: i128) {
        // Lifecycle is enforced at compile time; the TTL is auto-bumped to the
        // policy target on every access.
        BalanceKey::update(&env, &user, |bal| bal.unwrap_or(0) + amount);
    }

    pub fn balance(env: Env, user: Address) -> i128 {
        BalanceKey::get(&env, &user).unwrap_or(0)
    }
}
```

The `storage!` macro gives you:

- **Explicit lifecycle declarations** — every entry declares `persistent`,
  `temporary`, or `instance`. There is no way to read or write a key through the
  wrong lifecycle.
- **Auto-bump on access** — `get` / `set` / `update` extend the entry TTL
  according to a per-key `TtlPolicy` (check-then-extend, so recently touched
  entries are not re-bumped on every read).
- **Compile-time protection of critical data** — this does not compile:

  ```rust,ignore
  storage! {
      #[storage(temporary, critical)]
      Balance(Address) -> i128,  // error: critical entry declared `temporary`
  }
  ```
- **Explicit TTLs for bounded data** — `set_ttl(key, val, ttl_ledgers)` pins a
  lifetime for time-bounded state (claims, offers, grants).

## Linting

```bash
# Install the linter (cargo auto-discovers it as `cargo soroban-lint`)
cargo install --path crates/soroban-storage-lints

# Lint a contract's source; errors fail the build
cargo soroban-lint src --deny-warnings
```

Rules (see [docs/lint-rules.md](docs/lint-rules.md)):

| Rule | Severity | Flags |
|---|---|---|
| `temporary_critical_type` | error | critical data written through `env.storage().temporary()` |
| `lifecycle_mismatch` | error | `storage!`-declared key accessed via the wrong lifecycle |
| `missing_ttl_extension` | warning | write paths that never extend the entry TTL |
| `instance_storage_bloat` | warning | unbounded / loop-written values in instance storage |
| `temporary_ttl_exceeded` | warning | temporary extensions beyond the network max TTL (host clamps silently) |

The linter is a syntactic analyzer (Slither-style): it runs in milliseconds on
source without building the contract, and emits GitHub Actions annotations when
run in CI.

## CI

The packaged [action](.github/actions/soroban-storage-check/action.yml) installs
the toolchain, runs the linter (failing on errors), and builds the contract
wasm. Example workflow in
[`.github/workflows/storage-lint.yml`](.github/workflows/storage-lint.yml).

## Development

```bash
cargo test --workspace        # host tests: framework + example contract
cargo run -q -p soroban-storage-lints -- --deny-warnings crates/example-contract/src
cargo build --release --target wasm32v1-none -p example-contract
```

The toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml)
(verified against `1.98.1`; soroban-sdk 27 needs Rust 1.81+, and the
`wasm32v1-none` target needs 1.84+).

## Documentation

- [**Storage lifecycle guide**](docs/storage-guide.md) — storage types,
  archival semantics, TTL policies, and what the framework enforces.
- [**Lint rules reference**](docs/lint-rules.md) — every rule with examples,
  rationale, and how to opt out.

## Scope

This repository is the *on-chain & static-analysis layer*: the crate, the
linter, and the CI gate. The companion **rent-keeper daemon** (watches deployed
contract TTLs via RPC and submits `ExtendFootprintTTLOp` batches) and the
**restore-planner CLI** live in the sibling `soroban-rent-keeper` repository.

## License

MIT. See [LICENSE-MIT](LICENSE-MIT).