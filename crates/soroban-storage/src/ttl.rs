//! TTL policies and ledger arithmetic.

use crate::lifecycle::Lifecycle;

/// Number of ledgers in one day at the current ~5-second ledger cadence.
pub const LEDGERS_PER_DAY: u32 = 17_280;

/// Minimum TTL (in ledgers) a restored persistent entry receives:
/// `current ledger + 4095`. Network parameter — subject to upgrade.
pub const MIN_PERSISTENT_TTL: u32 = 4_095;

/// Maximum TTL a persistent entry can be extended to (~365 days). Network
/// parameter — subject to upgrade; the host clamps extension requests above it.
pub const MAX_PERSISTENT_TTL: u32 = 6_311_390;

/// Maximum TTL a temporary entry can be extended to (~180 days). Network
/// parameter — subject to upgrade; the host clamps extension requests above it.
pub const MAX_TEMP_TTL: u32 = 3_110_400;

/// How long an entry lives, expressed as TTL thresholds for `extend_ttl`.
///
/// `extend_ttl(key, threshold, extend_to)` extends an entry's TTL to
/// `extend_to` ledgers **only if** its current TTL is below `threshold`
/// ledgers. This check-then-extend shape is what makes bump-on-access cheap:
/// entries that were recently touched are not re-extended on every read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TtlPolicy {
    /// Only extend when the current TTL is below this many ledgers.
    pub threshold_ledgers: u32,
    /// Extend the TTL to at least this many ledgers (host-clamped to the
    /// lifecycle maximum).
    pub extend_to_ledgers: u32,
}

impl TtlPolicy {
    /// A policy with explicit ledger thresholds.
    pub const fn new(threshold_ledgers: u32, extend_to_ledgers: u32) -> Self {
        TtlPolicy {
            threshold_ledgers,
            extend_to_ledgers,
        }
    }

    /// A policy expressed in days (at ~5s ledgers, `17_280` ledgers/day).
    ///
    /// ```ignore
    /// use soroban_storage::TtlPolicy;
    /// const WEEKLY: TtlPolicy = TtlPolicy::days(7, 30);
    /// ```
    pub const fn days(threshold_days: u32, extend_days: u32) -> Self {
        TtlPolicy::new(
            threshold_days * LEDGERS_PER_DAY,
            extend_days * LEDGERS_PER_DAY,
        )
    }

    /// Clamps `extend_to_ledgers` to the maximum allowed for `lifecycle`.
    ///
    /// The host clamps anyway; this makes the policy predictable and lets the
    /// linter reason about extension sizes without knowing network state.
    pub const fn clamped_for(self, lifecycle: Lifecycle) -> Self {
        let max = match lifecycle {
            Lifecycle::Temporary => MAX_TEMP_TTL,
            Lifecycle::Persistent | Lifecycle::Instance => MAX_PERSISTENT_TTL,
        };
        let extend_to = if self.extend_to_ledgers > max {
            max
        } else {
            self.extend_to_ledgers
        };
        TtlPolicy {
            threshold_ledgers: self.threshold_ledgers,
            extend_to_ledgers: extend_to,
        }
    }
}

impl Default for TtlPolicy {
    /// The default bump policy: re-extend when TTL drops below 7 days, to 30
    /// days. Suitable for data accessed on a roughly weekly cadence.
    fn default() -> Self {
        TtlPolicy::days(7, 30)
    }
}

/// The default policy for a given lifecycle.
///
/// - `Temporary`: re-extend below 1 day, extend to 30 days.
/// - `Persistent` / `Instance`: re-extend below 7 days, extend to 30 days.
pub const fn default_policy(lifecycle: Lifecycle) -> TtlPolicy {
    match lifecycle {
        Lifecycle::Temporary => TtlPolicy::new(LEDGERS_PER_DAY, 30 * LEDGERS_PER_DAY),
        Lifecycle::Persistent | Lifecycle::Instance => {
            TtlPolicy::new(7 * LEDGERS_PER_DAY, 30 * LEDGERS_PER_DAY)
        }
    }
}
