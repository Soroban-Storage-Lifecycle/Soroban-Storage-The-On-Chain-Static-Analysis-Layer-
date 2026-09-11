//! The [`Rpc`] abstraction between the planner and the Stellar network.
//!
//! The planner is written against this trait so the planning flow is fully
//! testable against fakes. The production implementation is [`super::stellar`].

use stellar_xdr as xdr;

use crate::plan::Simulation;

/// The network surface the planner needs.
///
/// `async fn` in a trait is used deliberately: this trait is only implemented
/// and consumed inside this crate, so the auto-trait limitations do not matter.
#[allow(async_fn_in_trait)]
pub trait Rpc: Send + Sync {
    /// The current ledger sequence number.
    async fn latest_ledger(&self) -> Result<u32, String>;

    /// The current sequence number of `account` (a `G...` strkey).
    async fn account_sequence(&self, account: &str) -> Result<i64, String>;

    /// Simulates a probe restore transaction and returns the resource data for
    /// the entries that actually require restoration.
    async fn simulate_restore(&self, probe: &xdr::Transaction) -> Result<Simulation, String>;

    /// Sign-and-submits a base64-XDR transaction envelope, returning the
    /// transaction hash on success.
    async fn submit_envelope(&self, envelope_base64: &str) -> Result<String, String>;

    /// Resolves the contract's wasm hash from its (live) instance entry, when
    /// the instance entry is available.
    async fn fetch_wasm_hash(&self, contract_id: [u8; 32]) -> Result<Option<[u8; 32]>, String>;
}
