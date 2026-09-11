//! The keeper: poll entries, classify risk, submit batched extensions.

use crate::metrics::Metrics;
use crate::risk::{self, BatchEntry, RiskLevel};
use crate::rpc::{ExtendOutcome, Rpc, WatchKey};

/// What to watch for one contract: resolved ledger keys plus the extension
/// policy. Building the keys (instance, code, data) is the
/// [`crate::stellar`] implementation's job.
#[derive(Clone, Debug)]
pub struct WatchConfig {
    pub label: String,
    pub threshold_ledgers: u32,
    pub extend_to_ledgers: u32,
    pub keys: Vec<WatchKey>,
}

/// Outcome of one poll of one contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PollReport {
    pub watch: String,
    pub checked: usize,
    pub critical: usize,
    pub extended_batches: usize,
    pub submitted: usize,
}

/// The rent keeper. Cheap to construct; drive with [`Keeper::poll_once`] or
/// [`Keeper::run`].
pub struct Keeper<R: Rpc> {
    rpc: R,
    watches: Vec<WatchConfig>,
    metrics: Metrics,
    max_batch_bytes: u32,
}

impl<R: Rpc> Keeper<R> {
    pub fn new(rpc: R, watches: Vec<WatchConfig>, metrics: Metrics, max_batch_bytes: u32) -> Self {
        Keeper {
            rpc,
            watches,
            metrics,
            max_batch_bytes,
        }
    }

    /// One poll of every watched contract. Returns one result per watch.
    pub async fn poll_once(&self) -> Vec<Result<PollReport, String>> {
        let current = match self.rpc.latest_ledger().await {
            Ok(ledger) => ledger,
            Err(e) => return vec![Err(format!("latest_ledger: {e}"))],
        };
        let mut reports = Vec::new();
        for watch in &self.watches {
            match self.poll_watch(watch, current).await {
                Ok(report) => reports.push(Ok(report)),
                Err(e) => reports.push(Err(format!("{}: {e}", watch.label))),
            }
        }
        reports
    }

    async fn poll_watch(
        &self,
        watch: &WatchConfig,
        current_ledger: u32,
    ) -> Result<PollReport, String> {
        let snapshots = self.rpc.fetch_entries(&watch.keys).await?;
        self.metrics
            .set_entries_watched(&watch.label, snapshots.len() as u64);

        let mut due: Vec<BatchEntry> = Vec::new();
        let mut critical = 0usize;
        for snapshot in &snapshots {
            let ttl = risk::ttl_remaining(snapshot.live_until_ledger, current_ledger);
            let level = risk::classify(ttl, watch.threshold_ledgers, watch.extend_to_ledgers);
            self.metrics
                .set_ttl_gauge(&watch.label, &snapshot.label, ttl);
            if level == RiskLevel::Critical {
                critical += 1;
            }
            if risk::should_extend(ttl, watch.threshold_ledgers) {
                due.push(BatchEntry {
                    key_xdr: snapshot.key_xdr.clone(),
                    size_bytes: snapshot.size_bytes,
                });
            }
        }
        self.metrics.set_last_poll(&watch.label);

        if due.is_empty() {
            self.metrics.inc_extensions(&watch.label, "noop");
            return Ok(PollReport {
                watch: watch.label.clone(),
                checked: snapshots.len(),
                critical,
                extended_batches: 0,
                submitted: 0,
            });
        }

        let mut extended_batches = 0usize;
        let mut submitted = 0usize;
        for batch in risk::plan_batches(&due, self.max_batch_bytes) {
            let keys: Vec<String> = batch.iter().map(|e| e.key_xdr.clone()).collect();
            match self
                .rpc
                .extend_entries(&keys, watch.extend_to_ledgers)
                .await
            {
                Ok(ExtendOutcome::Submitted { .. }) => {
                    submitted += 1;
                    self.metrics.inc_extensions(&watch.label, "submitted");
                }
                Ok(ExtendOutcome::NoOp) => self.metrics.inc_extensions(&watch.label, "noop"),
                Err(_) => self.metrics.inc_extensions(&watch.label, "error"),
            }
            extended_batches += 1;
        }

        Ok(PollReport {
            watch: watch.label.clone(),
            checked: snapshots.len(),
            critical,
            extended_batches,
            submitted,
        })
    }

    /// Runs `poll_once` on a fixed interval, logging each report.
    pub async fn run(&self, poll_interval: std::time::Duration) {
        loop {
            let reports = self.poll_once().await;
            for report in reports {
                match report {
                    Ok(r) => eprintln!(
                        "poll [{}]: checked={} critical={} batches={} submitted={}",
                        r.watch, r.checked, r.critical, r.extended_batches, r.submitted
                    ),
                    Err(e) => eprintln!("poll error: {e}"),
                }
            }
            tokio::time::sleep(poll_interval).await;
        }
    }
}
