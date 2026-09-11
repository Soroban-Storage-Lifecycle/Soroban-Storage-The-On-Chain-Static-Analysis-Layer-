//! Prometheus metrics and the `/metrics` HTTP endpoint.

use std::net::TcpListener;
use std::thread;

use prometheus::{Encoder, IntCounterVec, IntGaugeVec, Opts, Registry, TextEncoder};

/// Metrics exposed by the keeper, labeled by contract and entry.
#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    ttl_ledgers: IntGaugeVec,
    extensions_total: IntCounterVec,
    last_poll_ledger: IntGaugeVec,
    entries_watched: IntGaugeVec,
}

impl Metrics {
    pub fn new() -> Self {
        let ttl_ledgers = IntGaugeVec::new(
            Opts::new(
                "soroban_rent_keeper_ttl_ledgers",
                "TTL (ledgers until eviction) of a watched entry",
            ),
            &["contract", "entry"],
        )
        .unwrap();
        let extensions_total = IntCounterVec::new(
            Opts::new(
                "soroban_rent_keeper_extensions_total",
                "ExtendFootprintTTLOp submissions by outcome (submitted|noop|error)",
            ),
            &["contract", "outcome"],
        )
        .unwrap();
        let last_poll_ledger = IntGaugeVec::new(
            Opts::new(
                "soroban_rent_keeper_last_poll_timestamp",
                "Unix timestamp of the last successful poll",
            ),
            &["contract"],
        )
        .unwrap();
        let entries_watched = IntGaugeVec::new(
            Opts::new(
                "soroban_rent_keeper_entries_watched",
                "Number of entries being watched per contract",
            ),
            &["contract"],
        )
        .unwrap();
        let registry = Registry::new();
        registry.register(Box::new(ttl_ledgers.clone())).unwrap();
        registry
            .register(Box::new(extensions_total.clone()))
            .unwrap();
        registry
            .register(Box::new(last_poll_ledger.clone()))
            .unwrap();
        registry
            .register(Box::new(entries_watched.clone()))
            .unwrap();
        Metrics {
            registry,
            ttl_ledgers,
            extensions_total,
            last_poll_ledger,
            entries_watched,
        }
    }

    pub fn set_ttl_gauge(&self, contract: &str, entry: &str, ttl: u32) {
        self.ttl_ledgers
            .with_label_values(&[contract, entry])
            .set(i64::from(ttl));
    }

    pub fn inc_extensions(&self, contract: &str, outcome: &str) {
        self.extensions_total
            .with_label_values(&[contract, outcome])
            .inc();
    }

    pub fn set_last_poll(&self, contract: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.last_poll_ledger
            .with_label_values(&[contract])
            .set(now);
    }

    pub fn set_entries_watched(&self, contract: &str, count: u64) {
        self.entries_watched
            .with_label_values(&[contract])
            .set(count as i64);
    }

    /// The Prometheus text-format exposition of all metrics.
    pub fn gather_text(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut out = Vec::new();
        if encoder.encode(&metric_families, &mut out).is_err() {
            return String::new();
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics::new()
    }
}

/// Serves the Prometheus text exposition on `addr` (e.g. `127.0.0.1:9090`)
/// until the process exits. Blocks the calling thread.
pub fn serve(addr: &str, metrics: Metrics) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("metrics bind {addr}: {e}"))?;
    eprintln!("metrics listening on http://{addr}/metrics");
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let body = metrics.gather_text();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
    }
    Ok(())
}

/// Spawns the metrics server on a background thread.
pub fn spawn_server(addr: &str, metrics: Metrics) -> thread::JoinHandle<()> {
    let addr = addr.to_string();
    thread::spawn(move || {
        if let Err(e) = serve(&addr, metrics) {
            eprintln!("metrics server error: {e}");
        }
    })
}
