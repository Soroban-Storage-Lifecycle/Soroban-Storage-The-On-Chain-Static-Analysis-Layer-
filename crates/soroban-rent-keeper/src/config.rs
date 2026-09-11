//! Configuration for the rent-keeper daemon.

use serde::Deserialize;

/// One contract to watch.
#[derive(Clone, Debug, Deserialize)]
pub struct ContractConfig {
    /// Contract id as a `C...` strkey.
    pub contract_id: String,
    /// Extend entries when their TTL drops to at most this many ledgers.
    /// Default: 10 days (~172_800 ledgers).
    #[serde(default = "default_threshold")]
    pub threshold_ledgers: u32,
    /// Extend entries to at least this many ledgers. Default: 30 days
    /// (~518_400 ledgers).
    #[serde(default = "default_extend_to")]
    pub extend_to_ledgers: u32,
    /// Additional contract-data ledger keys (base64-XDR `LedgerKey`) to watch.
    /// The contract instance and code entries are always watched.
    #[serde(default)]
    pub data_keys: Vec<String>,
}

fn default_threshold() -> u32 {
    172_800
}

fn default_extend_to() -> u32 {
    518_400
}

/// The daemon configuration (JSON).
#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    /// Stellar RPC endpoint, e.g. `https://soroban-testnet.stellar.org`.
    pub rpc_url: String,
    /// Network passphrase (must match the RPC network).
    pub network_passphrase: String,
    /// Signing account secret key (`S...` strkey) that pays for extensions.
    pub signer_secret: String,
    /// `host:port` for the Prometheus `/metrics` endpoint.
    #[serde(default = "default_metrics_addr")]
    pub metrics_addr: String,
    /// Upper bound on one `ExtendFootprintTTLOp` footprint size in bytes.
    #[serde(default = "default_max_batch_bytes")]
    pub max_batch_bytes: u32,
    /// Seconds between polls.
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    pub contracts: Vec<ContractConfig>,
}

fn default_metrics_addr() -> String {
    "127.0.0.1:9090".to_string()
}

fn default_max_batch_bytes() -> u32 {
    200_000
}

fn default_poll_interval_secs() -> u64 {
    600
}

impl Config {
    /// Loads and validates the configuration file. `rpc_url` and
    /// `signer_secret` can be overridden by the `SOROBAN_RPC_URL` and
    /// `SOROBAN_SIGNER_SECRET` environment variables (secrets should not live
    /// in committed files).
    pub fn load(path: &str) -> Result<Self, String> {
        let raw =
            std::fs::read_to_string(path).map_err(|e| format!("cannot read config {path}: {e}"))?;
        let mut config: Config =
            serde_json::from_str(&raw).map_err(|e| format!("invalid config {path}: {e}"))?;
        if let Ok(url) = std::env::var("SOROBAN_RPC_URL") {
            config.rpc_url = url;
        }
        if let Ok(secret) = std::env::var("SOROBAN_SIGNER_SECRET") {
            config.signer_secret = secret;
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.rpc_url.is_empty() {
            return Err("rpc_url is required".to_string());
        }
        if self.network_passphrase.is_empty() {
            return Err("network_passphrase is required".to_string());
        }
        if self.signer_secret.is_empty() {
            return Err("signer_secret is required (or set SOROBAN_SIGNER_SECRET)".to_string());
        }
        if self.contracts.is_empty() {
            return Err("at least one contract is required".to_string());
        }
        for c in &self.contracts {
            if c.contract_id.is_empty() {
                return Err("contract_id is required for every contract".to_string());
            }
            if c.threshold_ledgers >= c.extend_to_ledgers {
                return Err(format!(
                    "contract {}: threshold_ledgers ({}) must be < extend_to_ledgers ({})",
                    c.contract_id, c.threshold_ledgers, c.extend_to_ledgers
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "rpc_url": "https://rpc",
                "network_passphrase": "Test",
                "signer_secret": "S",
                "contracts": [{
                    "contract_id": "C",
                    "threshold_ledgers": 100,
                    "extend_to_ledgers": 500,
                    "data_keys": ["AAAA"]
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.contracts[0].extend_to_ledgers, 500);
        assert_eq!(cfg.metrics_addr, "127.0.0.1:9090");
        assert_eq!(cfg.poll_interval_secs, 600);
    }

    #[test]
    fn rejects_inverted_policy() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "rpc_url": "https://rpc",
                "network_passphrase": "Test",
                "signer_secret": "S",
                "contracts": [{
                    "contract_id": "C",
                    "threshold_ledgers": 500,
                    "extend_to_ledgers": 100
                }]
            }"#,
        )
        .unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn requires_at_least_one_contract() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "rpc_url": "https://rpc",
                "network_passphrase": "Test",
                "signer_secret": "S",
                "contracts": []
            }"#,
        )
        .unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn shipped_example_config_loads() {
        // Guards the documented sample against schema drift. Cargo runs tests
        // with the crate root as the working directory.
        let cfg = Config::load("config.example.json").expect("example config is valid");
        assert_eq!(cfg.contracts.len(), 1);
        assert!(cfg.contracts[0].threshold_ledgers < cfg.contracts[0].extend_to_ledgers);
    }
}
