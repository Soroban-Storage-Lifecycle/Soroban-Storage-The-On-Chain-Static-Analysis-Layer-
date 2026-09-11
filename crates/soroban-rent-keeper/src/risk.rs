//! Eviction-risk classification and extension batching.
//!
//! Everything here is pure and unit-tested; the keeper only wires these
//! functions to the network.

/// How close an entry is to eviction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// TTL is comfortably above the extension target.
    Safe,
    /// TTL is inside the policy envelope; the next poll may extend it.
    Watch,
    /// TTL is at or below the extension threshold — extend on the next poll.
    Critical,
}

impl RiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            RiskLevel::Safe => "safe",
            RiskLevel::Watch => "watch",
            RiskLevel::Critical => "critical",
        }
    }
}

/// TTL of a single watched entry, as seen from the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryTtl {
    /// `live_until_ledger` of the entry.
    pub live_until_ledger: u32,
    /// Serialized size in bytes (used for batch sizing / disk read resources).
    pub size_bytes: u32,
}

/// Ledgers remaining until `live_until_ledger`.
pub fn ttl_remaining(live_until_ledger: u32, current_ledger: u32) -> u32 {
    live_until_ledger.saturating_sub(current_ledger)
}

/// Classifies the eviction risk of an entry.
///
/// - `Critical`: TTL <= `threshold_ledgers` — the keeper must extend now.
/// - `Watch`: threshold < TTL < `extend_to_ledgers` — inside the extension
///   envelope; will be extended by the normal poll cadence.
/// - `Safe`: TTL >= `extend_to_ledgers` — the policy target is already met.
pub fn classify(ttl: u32, threshold_ledgers: u32, extend_to_ledgers: u32) -> RiskLevel {
    if ttl <= threshold_ledgers {
        RiskLevel::Critical
    } else if ttl < extend_to_ledgers {
        RiskLevel::Watch
    } else {
        RiskLevel::Safe
    }
}

/// Whether the keeper should extend this entry on the next poll.
pub fn should_extend(ttl: u32, threshold_ledgers: u32) -> bool {
    ttl <= threshold_ledgers
}

/// Ledgers until the entry crosses into `Critical` (can be negative if already
/// past it — expressed as `i64` for metrics/logging).
pub fn ledgers_until_critical(ttl: u32, threshold_ledgers: u32) -> i64 {
    i64::from(ttl) - i64::from(threshold_ledgers)
}

/// Whether the signer account can still pay for extensions.
pub fn signer_balance_is_sufficient(balance_stroops: i64, floor_stroops: i64) -> bool {
    balance_stroops >= floor_stroops
}

/// An entry queued for extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchEntry {
    /// The base64-XDR `LedgerKey` to include in the footprint.
    pub key_xdr: String,
    pub size_bytes: u32,
}

/// Partitions entries into `ExtendFootprintTTLOp` batches.
///
/// A single extension transaction carries its footprint in `read_only`; the
/// network enforces per-transaction size limits, so oversized batches are
/// split. The serialized size of the ledger keys (plus XDR overhead) is the
/// dominant cost; `max_batch_bytes` is a conservative cap on the total.
pub fn plan_batches(entries: &[BatchEntry], max_batch_bytes: u32) -> Vec<Vec<BatchEntry>> {
    let mut batches: Vec<Vec<BatchEntry>> = Vec::new();
    let mut current: Vec<BatchEntry> = Vec::new();
    let mut current_bytes: u32 = 0;
    for entry in entries {
        if !current.is_empty() && current_bytes.saturating_add(entry.size_bytes) > max_batch_bytes {
            batches.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(entry.size_bytes);
        current.push(entry.clone());
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_remaining_is_saturating() {
        assert_eq!(ttl_remaining(100, 90), 10);
        assert_eq!(ttl_remaining(90, 100), 0);
        assert_eq!(ttl_remaining(90, 500), 0);
    }

    #[test]
    fn classify_windows() {
        // threshold 172_800 (~10 days), extend_to 518_400 (~30 days)
        assert_eq!(classify(518_400, 172_800, 518_400), RiskLevel::Safe);
        assert_eq!(classify(500_000, 172_800, 518_400), RiskLevel::Watch);
        assert_eq!(classify(172_800, 172_800, 518_400), RiskLevel::Critical);
        assert_eq!(classify(10, 172_800, 518_400), RiskLevel::Critical);
        assert_eq!(classify(0, 172_800, 518_400), RiskLevel::Critical);
    }

    #[test]
    fn should_extend_threshold_boundary() {
        assert!(should_extend(172_800, 172_800));
        assert!(should_extend(10, 172_800));
        assert!(!should_extend(172_801, 172_800));
    }

    #[test]
    fn ledgers_until_critical_is_signed() {
        assert_eq!(ledgers_until_critical(200_000, 172_800), 27_200);
        assert_eq!(ledgers_until_critical(100_000, 172_800), -72_800);
    }

    #[test]
    fn batches_respect_size_cap() {
        let entries: Vec<BatchEntry> = (0..5)
            .map(|i| BatchEntry {
                key_xdr: format!("key{i}"),
                size_bytes: 1_000,
            })
            .collect();
        // Cap of 2_500 → batches of [2, 2, 1].
        let batches = plan_batches(&entries, 2_500);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(batches[1].len(), 2);
        assert_eq!(batches[2].len(), 1);
        // Total entries preserved.
        let total: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total, entries.len());
    }

    #[test]
    fn single_entry_larger_than_cap_gets_its_own_batch() {
        let entries = vec![BatchEntry {
            key_xdr: "big".into(),
            size_bytes: 10_000,
        }];
        let batches = plan_batches(&entries, 2_500);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn empty_input_yields_no_batches() {
        assert!(plan_batches(&[], 10_000).is_empty());
    }

    #[test]
    fn signer_balance_floor() {
        assert!(signer_balance_is_sufficient(10_000_000, 10_000_000));
        assert!(signer_balance_is_sufficient(1, 0));
        assert!(!signer_balance_is_sufficient(9_999_999, 10_000_000));
    }
}
