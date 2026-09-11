//! Integration tests for the keeper poll loop, driven by a fake [`Rpc`].

use std::sync::Mutex;

use soroban_rent_keeper::keeper::{Keeper, WatchConfig};
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
async fn a_watch_with_no_live_entries_is_a_noop() {
    let rpc = FakeRpc::new(vec![]);
    let keeper = Keeper::new(rpc, vec![watch(&["a"])], Metrics::new(), 1_000_000);

    let reports = keeper.poll_once().await;
    let report = reports[0].as_ref().unwrap();
    assert_eq!(report.checked, 0);
    assert_eq!(report.submitted, 0);
}
