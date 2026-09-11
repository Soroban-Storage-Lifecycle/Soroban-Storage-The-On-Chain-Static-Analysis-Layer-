//! Production [`Rpc`] implementation on top of `stellar-rpc-client` and
//! `stellar-xdr`.
//!
//! Strkey decoding, ledger-key construction and signing live in
//! [`soroban_stellar_common`], shared with the rent keeper.
//!
//! # Caveat
//!
//! The simulate → build → sign → submit flow follows the documented
//! `RestoreFootprintOp` pattern, but it is not exercised against a live
//! network in the test suite. Verify one restore end-to-end against testnet
//! before relying on it in automation.

use soroban_stellar_common::{contract_instance_key, vecm, wasm_hash_from_entry};
use stellar_rpc_client::Client;
use stellar_xdr as xdr;
use stellar_xdr::{Limits, ReadXdr};

use crate::plan::Simulation;
use crate::rpc::Rpc;

// Signing is security-critical and shared with the rent keeper; re-exported so
// the planner's public API is unchanged.
pub use soroban_stellar_common::{account_from_secret, sign_envelope};

/// The planner's network implementation.
pub struct StellarRpc {
    client: Client,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl StellarRpc {
    pub fn new(rpc_url: &str) -> Result<Self, String> {
        let client = Client::new(rpc_url).map_err(err)?;
        Ok(StellarRpc { client })
    }

    /// Fails unless the RPC network matches `network_passphrase`.
    pub async fn verify_network(&self, network_passphrase: &str) -> Result<(), String> {
        let actual = self
            .client
            .verify_network_passphrase(Some(network_passphrase))
            .await
            .map_err(err)?;
        if actual != network_passphrase {
            return Err(format!(
                "network passphrase mismatch: RPC says `{actual}`, expected `{network_passphrase}`"
            ));
        }
        Ok(())
    }
}

impl Rpc for StellarRpc {
    async fn latest_ledger(&self) -> Result<u32, String> {
        self.client
            .get_latest_ledger()
            .await
            .map(|r| r.sequence)
            .map_err(err)
    }

    async fn account_sequence(&self, account: &str) -> Result<i64, String> {
        let entry = self.client.get_account(account).await.map_err(err)?;
        Ok(entry.seq_num.0)
    }

    async fn simulate_restore(&self, probe: &xdr::Transaction) -> Result<Simulation, String> {
        let envelope = xdr::TransactionEnvelope::Tx(xdr::TransactionV1Envelope {
            tx: probe.clone(),
            signatures: vecm(Vec::new())?,
        });
        let sim = self
            .client
            .simulate_transaction_envelope(&envelope, None)
            .await
            .map_err(err)?;
        if let Some(error) = &sim.error {
            return Err(format!("simulation failed: {error}"));
        }
        // When the probe footprint contains archived entries, the network
        // returns the authoritative restore footprint in `restorePreamble`.
        let (data_b64, min_resource_fee) = match &sim.restore_preamble {
            Some(preamble) => (preamble.transaction_data.clone(), preamble.min_resource_fee),
            None => (sim.transaction_data.clone(), sim.min_resource_fee),
        };
        let transaction_data =
            xdr::SorobanTransactionData::from_xdr_base64(&data_b64, Limits::none())
                .map_err(|e| format!("invalid simulation transaction data: {e}"))?;
        Ok(Simulation {
            transaction_data,
            min_resource_fee,
        })
    }

    async fn submit_envelope(&self, envelope_base64: &str) -> Result<String, String> {
        let envelope = xdr::TransactionEnvelope::from_xdr_base64(envelope_base64, Limits::none())
            .map_err(|e| format!("invalid envelope XDR: {e}"))?;
        let response = self
            .client
            .send_transaction_polling(&envelope)
            .await
            .map_err(err)?;
        if response.status == "SUCCESS" {
            Ok(response.tx_hash.unwrap_or_else(|| "unknown".to_string()))
        } else {
            Err(format!(
                "restore transaction failed with status `{}`",
                response.status
            ))
        }
    }

    async fn fetch_wasm_hash(&self, contract_id: [u8; 32]) -> Result<Option<[u8; 32]>, String> {
        let key = contract_instance_key(contract_id);
        let response = self.client.get_ledger_entries(&[key]).await.map_err(err)?;
        for result in response.entries.unwrap_or_default() {
            let entry =
                xdr::LedgerEntry::from_xdr_base64(&result.xdr, Limits::none()).map_err(err)?;
            if let Some(hash) = wasm_hash_from_entry(&entry) {
                return Ok(Some(hash));
            }
        }
        Ok(None)
    }
}

/// Re-exported so callers can format an account consistently.
pub use soroban_stellar_common::account_strkey as account_address;
