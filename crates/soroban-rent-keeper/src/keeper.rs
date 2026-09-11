//! The keeper: poll entries, classify risk, submit batched extensions.
//!
//! Network calls go through a small bounded-retry wrapper: transient RPC
//! failures (timeouts, connection resets, 5xx, rate limits) are retried with
//! exponential backoff, while permanent failures (simulation rejected the
//! footprint, bad key) surface immediately. Retries and exhausted polls are
//! exported as metrics so alerting can tell "the RPC is flaky" apart from
//! "the keeper is broken".

use std::future::Future;
use std::time::Duration;

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

/// Bounded exponential backoff for transient RPC failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts, including the first. `3` means one try plus two retries.
    pub max_attempts: u32,
    /// Delay before the first retry; doubled for each subsequent retry.
    pub base_delay: Duration,
    /// Upper bound on any single backoff delay.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// How many times an operation may be retried after its first attempt.
    pub fn retries(&self) -> u32 {
        self.max_attempts.saturating_sub(1)
    }

    /// Delay before retry number `attempt` (1-based), doubling each time and
    /// clamped to `max_delay`.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(10);
        self.base_delay
            .saturating_mul(1u32 << shift)
            .min(self.max_delay)
    }
}

/// Whether an RPC error looks transient and is therefore worth retrying.
///
/// This is a deliberately conservative string heuristic: an unrecognised error
/// is treated as permanent so a broken configuration fails fast instead of
/// spinning.
pub fn is_transient(error: &str) -> bool {
    const NEEDLES: [&str; 14] = [
        "timeout",
        "timed out",
        "connection",
        "unavailable",
        "temporarily",
        "rate limit",
        "too many requests",
        "429",
        "502",
        "503",
        "504",
        "reset",
        "broken pipe",
        "eof",
    ];
    let lower = error.to_ascii_lowercase();
    NEEDLES.iter().any(|needle| lower.contains(needle))
}

/// Runs `op`, retrying transient failures with bounded exponential backoff.
async fn with_retry<T, F, Fut>(
    policy: RetryPolicy,
    metrics: &Metrics,
    contract: &str,
    mut op: F,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let mut attempt = 0u32;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if attempt >= policy.retries() || !is_transient(&error) {
                    return Err(error);
                }
                attempt += 1;
                metrics.inc_retry(contract);
                tokio::time::sleep(policy.delay_for(attempt)).await;
            }
        }
    }
}

/// The rent keeper. Cheap to construct; drive with [`Keeper::poll_once`] or
/// [`Keeper::run`].
pub struct Keeper<R: Rpc> {
    rpc: R,
    watches: Vec<WatchConfig>,
    metrics: Metrics,
    max_batch_bytes: u32,
    retry: RetryPolicy,
}

impl<R: Rpc> Keeper<R> {
    pub fn new(rpc: R, watches: Vec<WatchConfig>, metrics: Metrics, max_batch_bytes: u32) -> Self {
        Keeper {
            rpc,
            watches,
            metrics,
            max_batch_bytes,
            retry: RetryPolicy::default(),
        }
    }

    /// Overrides the retry policy (builder style).
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// One poll of every watched contract. Returns one result per watch.
    pub async fn poll_once(&self) -> Vec<Result<PollReport, String>> {
        // Refresh the balance gauge each cycle; a failure here is not fatal
        // (the retry/error counters already surface RPC trouble).
        if let Ok(balance) = self.rpc.signer_balance_stroops().await {
            self.metrics.set_signer_balance(balance);
        }

        let current = match with_retry(self.retry, &self.metrics, "global", || {
            self.rpc.latest_ledger()
        })
        .await
        {
            Ok(ledger) => ledger,
            Err(e) => {
                self.metrics.inc_poll_error("global");
                return vec![Err(format!("latest_ledger: {e}"))];
            }
        };
        let mut reports = Vec::new();
        for watch in &self.watches {
            match self.poll_watch(watch, current).await {
                Ok(report) => reports.push(Ok(report)),
                Err(e) => {
                    self.metrics.inc_poll_error(&watch.label);
                    reports.push(Err(format!("{}: {e}", watch.label)));
                }
            }
        }
        reports
    }

    async fn poll_watch(
        &self,
        watch: &WatchConfig,
        current_ledger: u32,
    ) -> Result<PollReport, String> {
        let snapshots = with_retry(self.retry, &self.metrics, &watch.label, || {
            self.rpc.fetch_entries(&watch.keys)
        })
        .await?;
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
            let outcome = with_retry(self.retry, &self.metrics, &watch.label, || {
                self.rpc.extend_entries(&keys, watch.extend_to_ledgers)
            })
            .await;
            match outcome {
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
    pub async fn run(&self, poll_interval: Duration) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_is_capped() {
        let policy = RetryPolicy {
            max_attempts: 6,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
        };
        assert_eq!(policy.delay_for(1), Duration::from_millis(100));
        assert_eq!(policy.delay_for(2), Duration::from_millis(200));
        assert_eq!(policy.delay_for(3), Duration::from_millis(400));
        // Capped from the fourth retry on.
        assert_eq!(policy.delay_for(4), Duration::from_millis(500));
        assert_eq!(policy.delay_for(50), Duration::from_millis(500));
    }

    #[test]
    fn retries_exclude_the_first_attempt() {
        let policy = RetryPolicy {
            max_attempts: 3,
            ..RetryPolicy::default()
        };
        assert_eq!(policy.retries(), 2);
        let single = RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        };
        assert_eq!(single.retries(), 0);
    }

    #[test]
    fn classifies_transient_errors() {
        assert!(is_transient("request timeout"));
        assert!(is_transient("connection reset by peer"));
        assert!(is_transient("HTTP 503 Service Unavailable"));
        assert!(is_transient("too many requests"));
        assert!(!is_transient("simulation failed: insufficient footprint"));
        assert!(!is_transient("invalid ledger key"));
    }
}
