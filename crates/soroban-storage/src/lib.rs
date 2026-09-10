//! # soroban-storage
//!
//! A storage-lifecycle framework for Soroban smart contracts that makes the
//! *correct* TTL pattern the default and the dangerous one impossible.
//!
//! ## Why this exists
//!
//! Every persistent entry in a Soroban contract has a time-to-live and needs
//! rent paid or it gets evicted:
//!
//! - **Temporary** entries are **permanently deleted** when their TTL reaches
//!   zero. They can never be restored. Storing user balances here causes
//!   irreversible loss of funds.
//! - **Persistent** entries are archived when their TTL reaches zero and can be
//!   restored, but a restored entry only gets extended to the network minimum
//!   (currently `current ledger + 4095`).
//! - **Instance** storage shares the TTL of the contract instance and is the
//!   right place for shared, non-recreatable metadata.
//!
//! The framework gives you:
//!
//! 1. **Explicit lifecycle declarations** — every storage key is bound to
//!    exactly one lifecycle at compile time via [`storage!`]; there is no way to
//!    accidentally read or write a key through the wrong lifecycle.
//! 2. **Auto-bump TTLs on access** — `get`/`set`/`update` extend the entry TTL
//!    according to a per-key [`TtlPolicy`], so frequently-used data never
//!    drifts toward eviction.
//! 3. **Compile-time protection of critical data** — entries marked `critical`
//!    (user funds, balances, positions) are **rejected** if declared
//!    `temporary`, and the companion `soroban-storage-lints` static analysis
//!    flags the same mistake in raw `env.storage()` code.
//!
//! ## Quick start
//!
//! ```ignore
//! use soroban_sdk::{contract, contractimpl, Address, Env};
//! use soroban_storage::{storage, TtlPolicy};
//!
//! storage! {
//!     /// User token balance. Irreplaceable — never temporary.
//!     #[storage(persistent, critical)]
//!     #[storage(policy = TtlPolicy::days(7, 30))]
//!     Balance(Address) -> i128,
//!
//!     #[storage(temporary)]
//!     Nonce(Address) -> u64,
//! }
//!
//! #[contract]
//! pub struct Vault;
//!
//! #[contractimpl]
//! impl Vault {
//!     pub fn deposit(env: Env, user: Address, amount: i128) {
//!         // Auto-bumps the balance entry TTL on every access.
//!         BalanceKey::update(&env, &user, |bal| bal.unwrap_or(0) + amount);
//!     }
//!
//!     pub fn balance(env: Env, user: Address) -> i128 {
//!         BalanceKey::get(&env, &user).unwrap_or(0)
//!     }
//! }
//! ```
//!
//! ## The full framework
//!
//! - `crates/soroban-storage` — this crate.
//! - `crates/soroban-storage-lints` — `cargo`-integrated static analysis
//!   (`cargo soroban-lint`) flagging critical types in `temporary()`, missing
//!   TTL extensions on write paths, and instance-storage bloat.
//! - `.github/workflows/storage-lint.yml` — CI gate that fails builds on lint
//!   errors.
#![no_std]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod critical;
pub mod lifecycle;
pub mod ops;
pub mod prelude;
pub mod ttl;

// `Critical` is exported twice on purpose: the trait (type namespace) and the
// derive macro (macro namespace) share the name, exactly like `serde::Serialize`.
pub use critical::Critical;
pub use lifecycle::Lifecycle;
pub use soroban_storage_macros::storage;
pub use soroban_storage_macros::Critical;
pub use ttl::TtlPolicy;
