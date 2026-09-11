//! Integration tests for the keeper poll loop, driven by a fake [`Rpc`].

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use soroban_rent_keeper::keeper::{Keeper, RetryPolicy, WatchConfig};
use soroban_rent_keeper::metrics::Metrics;
use soroban_rent_keeper::rpc::{EntrySnapshot, ExtendOutcome, Rpc, WatchKey};

/// A fake network returning canned snapshots and recording extensions.
struct FakeRpc {
    current_ledger: u32,
    snapshots: Vec<EntrySnapshot>,
    extended: Mutex<Vec<(Vec<String>, u32)>>,
}

impl FakeRpc {
    fn new(snapshots: Vec<EntrySnapshot>) -> Self {
        FakeRpc {
            current_ledger: 1_000,
            snapshots,
            extended: Mutex::new(Vec::new()),
        }
    }
}

impl Rpc for FakeRpc {
    async fn latest_ledger(&self) -> Result<u32, String> {
        Ok(self.current_ledger)
    }

    async fn signer_balance_stroops(&self) -> Result<i64, String> {
        Ok(10_000_000)
    }

    async fn fetch_entries(&self, keys: &[WatchKey]) -> Result<Vec<EntrySnapshot>, String> {
        let wanted: std::collections::HashSet<&str> =
            keys.iter().map(|k| k.key_xdr.as_str()).collect();
        Ok(self
            .snapshots
            .iter()
            .filter(|s| wanted.contains(s.key_xdr.as_str()))
            .cloned()
            .collect())
    }

    async fn extend_entries(
        &self,
        keys: &[String],
        extend_to: u32,
    ) -> Result<ExtendOutcome, String> {
        self.extended
            .lock()
            .unwrap()
            .push((keys.to_vec(), extend_to));
        Ok(ExtendOutcome::Submitted {
            tx_hash: "abc".to_string(),
        })
    }
}

fn snapshot(label: &str, ttl: u32, size: u32) -> EntrySnapshot {
    EntrySnapshot {
        label: label.to_string(),
        key_xdr: format!("key:{label}"),
        live_until_ledger: 1_000 + ttl,
        size_bytes: size,
    }
}

fn watch(keys: &[&str]) -> WatchConfig {
    WatchConfig {
        label: "C...".to_string(),
        threshold_ledgers: 200,
        extend_to_ledgers: 2_000,
        keys: keys
            .iter()
            .map(|k| WatchKey {
                label: (*k).to_string(),
                key_xdr: format!("key:{k}"),
            })
            .collect(),
    }
}

#[tokio::test]
async fn critical_entries_are_extended() {
    let rpc = FakeRpc::new(vec![snapshot("a", 100, 300)]);
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], Metrics::new(), 1_000_000);

    let reports = keeper.poll_once().await;
    assert_eq!(reports.len(), 1);
    let report = reports[0].as_ref().unwrap();
    assert_eq!(report.checked, 1);
    assert_eq!(report.critical, 1);
    assert_eq!(report.submitted, 1);
}

#[tokio::test]
async fn safe_entries_are_not_extended() {
    let rpc = FakeRpc::new(vec![snapshot("a", 500_000, 300)]);
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], Metrics::new(), 1_000_000);

    let reports = keeper.poll_once().await;
    let report = reports[0].as_ref().unwrap();
    assert_eq!(report.critical, 0);
    assert_eq!(report.submitted, 0);
    assert_eq!(report.extended_batches, 0);
}

#[tokio::test]
async fn oversized_batches_are_split() {
    // Two entries of 400 bytes each with a 500-byte cap → two batches.
    let rpc = FakeRpc::new(vec![snapshot("a", 10, 400), snapshot("b", 10, 400)]);
    let keeper = Keeper::new(rpc, vec![watch(&["a", "b"])], Metrics::new(), 500);

    let reports = keeper.poll_once().await;
    let report = reports[0].as_ref().unwrap();
    assert_eq!(report.submitted, 2);
    assert_eq!(report.extended_batches, 2);
}

#[tokio::test]
async fn signer_balance_is_exported_each_poll() {
    let rpc = FakeRpc::new(vec![snapshot("a", 500_000, 300)]);
    let metrics = Metrics::new();
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], metrics.clone(), 1_000_000);

    keeper.poll_once().await;
    let text = metrics.gather_text();
    assert!(
        text.contains("soroban_rent_keeper_signer_balance_stroops"),
        "balance gauge missing from exposition: {text}"
    );
    assert!(text.contains("10000000"), "balance value missing: {text}");
}

#[tokio::test]
async fn a_watch_with_no_live_entries_is_a_noop() {
    let rpc = FakeRpc::new(vec![]);
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], Metrics::new(), 1_000_000);

    let reports = keeper.poll_once().await;
    let report = reports[0].as_ref().unwrap();
    assert_eq!(report.checked, 0);
    assert_eq!(report.submitted, 0);
}

/// Fails `fetch_entries` with `error` until `remaining_failures` reaches zero.
struct FailingRpc {
    snapshots: Vec<EntrySnapshot>,
    remaining_failures: AtomicUsize,
    error: &'static str,
}

impl Rpc for FailingRpc {
    async fn latest_ledger(&self) -> Result<u32, String> {
        Ok(1_000)
    }

    async fn signer_balance_stroops(&self) -> Result<i64, String> {
        Ok(10_000_000)
    }

    async fn fetch_entries(&self, _keys: &[WatchKey]) -> Result<Vec<EntrySnapshot>, String> {
        if self.remaining_failures.load(Ordering::SeqCst) > 0 {
            self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
            return Err(self.error.to_string());
        }
        Ok(self.snapshots.clone())
    }

    async fn extend_entries(
        &self,
        _keys: &[String],
        _extend_to: u32,
    ) -> Result<ExtendOutcome, String> {
        Ok(ExtendOutcome::Submitted {
            tx_hash: "abc".to_string(),
        })
    }
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(2),
    }
}

#[tokio::test]
async fn transient_failures_are_retried() {
    let rpc = FailingRpc {
        snapshots: vec![snapshot("a", 100, 300)],
        remaining_failures: AtomicUsize::new(2), // one try + two retries
        error: "connection reset by peer",
    };
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], Metrics::new(), 1_000_000)
        .with_retry_policy(fast_retry());

    let reports = keeper.poll_once().await;
    let report = reports[0].as_ref().expect("poll recovers after retries");
    assert_eq!(report.submitted, 1);
}

#[tokio::test]
async fn permanent_failures_surface_without_retrying() {
    let rpc = FailingRpc {
        snapshots: vec![snapshot("a", 100, 300)],
        remaining_failures: AtomicUsize::new(3),
        error: "simulation failed: invalid footprint",
    };
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], Metrics::new(), 1_000_000)
        .with_retry_policy(fast_retry());

    let reports = keeper.poll_once().await;
    assert!(reports[0].is_err(), "permanent errors must not be masked");
}

#[tokio::test]
async fn retries_are_exhausted_for_transient_errors() {
    let rpc = FailingRpc {
        snapshots: vec![snapshot("a", 100, 300)],
        remaining_failures: AtomicUsize::new(99),
        error: "connection timeout",
    };
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], Metrics::new(), 1_000_000)
        .with_retry_policy(fast_retry());

    let reports = keeper.poll_once().await;
    assert!(
        reports[0].is_err(),
        "a permanently unreachable RPC eventually fails"
    );
}
