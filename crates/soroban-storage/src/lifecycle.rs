//! The three Soroban storage lifecycles.

/// The storage lifecycle an entry is bound to.
///
/// Mirrors `env.storage().temporary()` / `.persistent()` / `.instance()` but as
/// a first-class, compile-time value so storage keys can be declared once and
/// enforced everywhere they are used.
///
/// # Archival semantics
///
/// | Lifecycle   | When TTL reaches zero                          | Restorable? |
/// |-------------|------------------------------------------------|-------------|
/// | `Temporary` | Entry is permanently deleted                   | Never       |
/// | `Persistent`| Entry is archived; accessible again when restored | Via restore list / `RestoreFootprintOp` |
/// | `Instance`  | Contract instance is archived (auto-restored on use) | Yes (with the instance) |
///
/// See <https://developers.stellar.org/docs/learn/fundamentals/contract-development/storage/state-archival>.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lifecycle {
    /// Cheapest storage; permanently deleted on expiry. Only for data that is
    /// easily recreated or time-bounded (oracles, signatures, nonces).
    Temporary,
    /// Per-entry TTL; archived (and restorable) on expiry. For user data and
    /// balances.
    Persistent,
    /// Stored inside the contract instance entry; shares the instance TTL.
    /// For shared contract metadata (admin, config).
    Instance,
}

impl Lifecycle {
    /// The identifier used by the Soroban SDK, e.g. `"persistent"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Lifecycle::Temporary => "temporary",
            Lifecycle::Persistent => "persistent",
            Lifecycle::Instance => "instance",
        }
    }

    /// Whether entries with this lifecycle can ever be restored after expiry.
    pub const fn is_restorable(self) -> bool {
        !matches!(self, Lifecycle::Temporary)
    }

    /// Whether this lifecycle is safe for critical, irreplaceable data.
    pub const fn accepts_critical(self) -> bool {
        !matches!(self, Lifecycle::Temporary)
    }
}
