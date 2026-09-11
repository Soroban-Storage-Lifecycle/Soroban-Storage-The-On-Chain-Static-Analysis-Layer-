//! The [`Rpc`] abstraction between the keeper and the Stellar network.
//!
//! The keeper is written against this trait so the poll/extend loop is fully
//! testable against fakes. The production implementation is [`super::stellar`].

/// A ledger key being watched, with a stable label for metrics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchKey {
    /// Human-readable label (e.g. `instance`, `code`, `data:0`).
    pub label: String,
    /// Base64-XDR `LedgerKey`.
    pub key_xdr: String,
}

/// A snapshot of one watched ledger entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntrySnapshot {
    pub label: String,
    /// Base64-XDR `LedgerKey` of the entry (used to rebuild the footprint).
    pub key_xdr: String,
    /// `live_until_ledger` of the entry.
    pub live_until_ledger: u32,
    /// Serialized size in bytes (batch sizing / disk-read resources).
    pub size_bytes: u32,
}

/// Result of an extension submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtendOutcome {
    /// Transaction accepted by the network (hash hex).
    Submitted { tx_hash: String },
    /// Nothing was extended (no keys, or already fresh).
    NoOp,
}

/// The network surface the keeper needs.
///
/// `async fn` in a trait is used deliberately: this trait is only implemented
/// and consumed inside this crate, so the auto-trait limitations do not matter.
#[allow(async_fn_in_trait)]
pub trait Rpc: Send + Sync {
    /// The current ledger sequence number.
    async fn latest_ledger(&self) -> Result<u32, String>;

    /// The signer account's current balance, in stroops, so the keeper can
    /// export it as a gauge on every poll.
    async fn signer_balance_stroops(&self) -> Result<i64, String>;

    /// Fetches the entries for the given ledger keys, returning snapshots with
    /// the corresponding labels (missing keys are skipped).
    async fn fetch_entries(&self, keys: &[WatchKey]) -> Result<Vec<EntrySnapshot>, String>;

    /// Extends the given entries' TTL to at least `extend_to` ledgers from now
    /// via a single `ExtendFootprintTTLOp` transaction.
    async fn extend_entries(
        &self,
        keys: &[String],
        extend_to: u32,
    ) -> Result<ExtendOutcome, String>;
}
