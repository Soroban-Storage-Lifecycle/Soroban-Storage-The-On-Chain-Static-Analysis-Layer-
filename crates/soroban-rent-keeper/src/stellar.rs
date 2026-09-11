//! Production [`Rpc`] implementation on top of `stellar-rpc-client` and
//! `stellar-xdr`.
//!
//! # Caveat
//!
//! The transaction build/simulate/sign/submit flow here follows the documented
//! `ExtendFootprintTTLOp` pattern (simulate → adopt `SorobanTransactionData` +
//! fee → sign → submit), but it is exercised without a live network in the
//! test suite. Before pointing the daemon at mainnet, run it against a testnet
//! RPC with a funded account and verify one extension end-to-end.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use stellar_rpc_client::Client;
use stellar_xdr::{Limits, ReadXdr, WriteXdr};
// stellar-xdr 27 exposes the current-protocol types at the crate root.
use stellar_xdr as xdr;

use crate::config::ContractConfig;
use crate::keeper::WatchConfig;
use crate::rpc::{EntrySnapshot, ExtendOutcome, Rpc, WatchKey};

/// The keeper's network implementation.
pub struct StellarRpc {
    client: Client,
    source_account: xdr::AccountId,
    signing_key: SigningKey,
    network_passphrase: String,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// A `VecM` from a `Vec`, with the length bound enforced by the XDR type.
fn vecm<T, const N: u32>(items: Vec<T>) -> Result<xdr::VecM<T, N>, String> {
    items.try_into().map_err(err)
}

fn keypair_from_secret(secret: &str) -> Result<(xdr::AccountId, SigningKey), String> {
    let strkey = stellar_strkey::Strkey::from_string(secret)
        .map_err(|e| format!("invalid signer secret: {e}"))?;
    let bytes = match strkey {
        stellar_strkey::Strkey::PrivateKeyEd25519(bytes) => bytes.0,
        _ => return Err("signer secret must be a private key strkey".to_string()),
    };
    let signing_key = SigningKey::from_bytes(&bytes);
    let verifying = signing_key.verifying_key().to_bytes();
    let account_id = xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(
        verifying,
    )));
    Ok((account_id, signing_key))
}

/// The `G...` strkey of an ed25519 account id.
fn account_strkey(account: &xdr::AccountId) -> String {
    match account {
        xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(h)) => {
            // `ed25519::PublicKey` has an inherent `to_string` returning a
            // heapless string, so go through `Display` for a std `String`.
            format!("{}", stellar_strkey::ed25519::PublicKey(h.0))
        }
    }
}

/// The transaction-level source account as a `MuxedAccount`.
fn muxed(account: &xdr::AccountId) -> xdr::MuxedAccount {
    match account {
        xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(h)) => {
            xdr::MuxedAccount::Ed25519(h.clone())
        }
    }
}

fn decode_contract_id(strkey: &str) -> Result<[u8; 32], String> {
    match stellar_strkey::Strkey::from_string(strkey).map_err(err)? {
        stellar_strkey::Strkey::Contract(bytes) => Ok(bytes.0),
        _ => Err(format!("{strkey} is not a contract id strkey")),
    }
}

fn ledger_key_instance(contract_id: [u8; 32]) -> xdr::LedgerKey {
    xdr::LedgerKey::ContractData(xdr::LedgerKeyContractData {
        contract: xdr::ScAddress::Contract(xdr::ContractId(xdr::Hash(contract_id))),
        key: xdr::ScVal::LedgerKeyContractInstance,
        durability: xdr::ContractDataDurability::Persistent,
    })
}

fn ledger_key_code(wasm_hash: [u8; 32]) -> xdr::LedgerKey {
    xdr::LedgerKey::ContractCode(xdr::LedgerKeyContractCode {
        hash: xdr::Hash(wasm_hash),
    })
}

fn entry_size_bytes(entry: &xdr::LedgerEntry) -> u32 {
    entry
        .to_xdr(Limits::none())
        .map(|b| b.len() as u32)
        .unwrap_or(300)
}

/// The SHA-256 signature payload for a V1 transaction: the `network_id`
/// (`sha256(passphrase)`) plus the tagged transaction, hashed again.
fn signature_payload(tx: &xdr::Transaction, passphrase: &str) -> Result<[u8; 32], String> {
    let network_id = Sha256::digest(passphrase.as_bytes());
    let payload = xdr::TransactionSignaturePayload {
        network_id: xdr::Hash(network_id.into()),
        tagged_transaction: xdr::TransactionSignaturePayloadTaggedTransaction::Tx(tx.clone()),
    };
    let bytes = payload.to_xdr(Limits::none()).map_err(err)?;
    Ok(Sha256::digest(bytes).into())
}

impl StellarRpc {
    pub fn new(
        rpc_url: &str,
        network_passphrase: &str,
        signer_secret: &str,
    ) -> Result<Self, String> {
        let client = Client::new(rpc_url).map_err(err)?;
        let (source_account, signing_key) = keypair_from_secret(signer_secret)?;
        Ok(StellarRpc {
            client,
            source_account,
            signing_key,
            network_passphrase: network_passphrase.to_string(),
        })
    }

    /// Verifies the RPC network matches the configured passphrase.
    pub async fn verify_network(&self) -> Result<(), String> {
        let actual = self
            .client
            .verify_network_passphrase(Some(&self.network_passphrase))
            .await
            .map_err(err)?;
        if actual != self.network_passphrase {
            return Err(format!(
                "network passphrase mismatch: RPC says `{actual}`, config says `{}`",
                self.network_passphrase
            ));
        }
        Ok(())
    }

    /// Resolves the ledger keys to watch for a contract: the instance entry
    /// (whose `wasm_hash` yields the code key) plus any configured data keys.
    pub async fn build_watch(&self, cc: &ContractConfig) -> Result<WatchConfig, String> {
        let contract_id = decode_contract_id(&cc.contract_id)?;
        let instance_key = ledger_key_instance(contract_id);
        let entries = self
            .client
            .get_ledger_entries(std::slice::from_ref(&instance_key))
            .await
            .map_err(err)?;

        let mut wasm_hash: Option<[u8; 32]> = None;
        for result in entries.entries.unwrap_or_default() {
            let entry =
                xdr::LedgerEntry::from_xdr_base64(&result.xdr, Limits::none()).map_err(err)?;
            if let xdr::LedgerEntryData::ContractData(d) = &entry.data {
                if let xdr::ScVal::ContractInstance(inst) = &d.val {
                    wasm_hash = match &inst.executable {
                        xdr::ContractExecutable::Wasm(h) => Some(h.0),
                        _ => None,
                    };
                }
            }
        }

        let mut keys = Vec::new();
        keys.push(WatchKey {
            label: "instance".to_string(),
            key_xdr: instance_key.to_xdr_base64(Limits::none()).map_err(err)?,
        });
        match wasm_hash {
            Some(hash) => {
                let code_key = ledger_key_code(hash);
                keys.push(WatchKey {
                    label: "code".to_string(),
                    key_xdr: code_key.to_xdr_base64(Limits::none()).map_err(err)?,
                });
            }
            None => {
                return Err(format!(
                    "{}: could not resolve wasm hash from the contract instance (is the contract deployed?)",
                    cc.contract_id
                ));
            }
        }
        for (i, key) in cc.data_keys.iter().enumerate() {
            keys.push(WatchKey {
                label: format!("data:{i}"),
                key_xdr: key.clone(),
            });
        }

        Ok(WatchConfig {
            label: cc.contract_id.clone(),
            threshold_ledgers: cc.threshold_ledgers,
            extend_to_ledgers: cc.extend_to_ledgers,
            keys,
        })
    }

    /// Builds an unsigned `ExtendFootprintTTLOp` transaction.
    fn build_extend_transaction(
        &self,
        seq_num: i64,
        current_ledger: u32,
        extend_to: u32,
        resources: xdr::SorobanTransactionData,
        fee: u32,
    ) -> Result<xdr::Transaction, String> {
        let op = xdr::Operation {
            source_account: None,
            body: xdr::OperationBody::ExtendFootprintTtl(xdr::ExtendFootprintTtlOp {
                ext: xdr::ExtensionPoint::V0,
                extend_to,
            }),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let preconditions = xdr::Preconditions::V2(xdr::PreconditionsV2 {
            time_bounds: Some(xdr::TimeBounds {
                min_time: xdr::TimePoint(0),
                max_time: xdr::TimePoint(now + 300),
            }),
            ledger_bounds: Some(xdr::LedgerBounds {
                min_ledger: current_ledger,
                max_ledger: current_ledger + 200,
            }),
            min_seq_num: None,
            min_seq_age: xdr::Duration(0),
            min_seq_ledger_gap: 0,
            extra_signers: vecm(Vec::new())?,
        });
        Ok(xdr::Transaction {
            source_account: muxed(&self.source_account),
            fee,
            seq_num: xdr::SequenceNumber(seq_num),
            cond: preconditions,
            memo: xdr::Memo::None,
            operations: vecm(vec![op])?,
            ext: xdr::TransactionExt::V1(resources),
        })
    }
}

impl Rpc for StellarRpc {
    async fn latest_ledger(&self) -> Result<u32, String> {
        let response = self.client.get_latest_ledger().await.map_err(err)?;
        Ok(response.sequence)
    }

    async fn fetch_entries(&self, keys: &[WatchKey]) -> Result<Vec<EntrySnapshot>, String> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let labels: std::collections::HashMap<&str, &str> = keys
            .iter()
            .map(|k| (k.key_xdr.as_str(), k.label.as_str()))
            .collect();
        let ledger_keys: Vec<xdr::LedgerKey> = keys
            .iter()
            .map(|k| xdr::LedgerKey::from_xdr_base64(&k.key_xdr, Limits::none()).map_err(err))
            .collect::<Result<_, _>>()?;
        let response = self
            .client
            .get_ledger_entries(&ledger_keys)
            .await
            .map_err(err)?;
        let mut snapshots = Vec::new();
        for result in response.entries.unwrap_or_default() {
            // TTL lives on the ledger entry *key* (the `TTLEntry`), which the
            // RPC reports alongside the entry, not inside the entry XDR.
            let Some(live_until_ledger) = result.live_until_ledger_seq_ledger_seq else {
                continue;
            };
            let entry =
                xdr::LedgerEntry::from_xdr_base64(&result.xdr, Limits::none()).map_err(err)?;
            let label = labels
                .get(result.key.as_str())
                .copied()
                .unwrap_or("entry")
                .to_string();
            snapshots.push(EntrySnapshot {
                label,
                key_xdr: result.key.clone(),
                live_until_ledger,
                size_bytes: entry_size_bytes(&entry),
            });
        }
        Ok(snapshots)
    }

    async fn extend_entries(
        &self,
        keys: &[String],
        extend_to: u32,
    ) -> Result<ExtendOutcome, String> {
        if keys.is_empty() {
            return Ok(ExtendOutcome::NoOp);
        }
        let ledger_keys: Vec<xdr::LedgerKey> = keys
            .iter()
            .map(|k| xdr::LedgerKey::from_xdr_base64(k, Limits::none()).map_err(err))
            .collect::<Result<_, _>>()?;

        let current_ledger = self.latest_ledger().await?;
        let account = self
            .client
            .get_account(&account_strkey(&self.source_account))
            .await
            .map_err(err)?;
        let next_seq = account.seq_num.0 + 1;

        // Read-only footprint carries the entries being extended; the RPC
        // simulation replaces these placeholder resources with accurate ones.
        let read_bytes: u32 = ledger_keys
            .iter()
            .map(|k| {
                k.to_xdr(Limits::none())
                    .map(|b| b.len() as u32)
                    .unwrap_or(300)
            })
            .sum();
        let placeholder = xdr::SorobanTransactionData {
            ext: xdr::SorobanTransactionDataExt::V0,
            resources: xdr::SorobanResources {
                footprint: xdr::LedgerFootprint {
                    read_only: vecm(ledger_keys.clone())?,
                    read_write: vecm(Vec::new())?,
                },
                instructions: 0,
                disk_read_bytes: read_bytes,
                write_bytes: 0,
            },
            resource_fee: 0,
        };

        let base_tx =
            self.build_extend_transaction(next_seq, current_ledger, extend_to, placeholder, 100)?;
        let envelope = xdr::TransactionEnvelope::Tx(xdr::TransactionV1Envelope {
            tx: base_tx,
            signatures: vecm(Vec::new())?,
        });

        // Simulate to obtain accurate resources and fees.
        let sim = self
            .client
            .simulate_transaction_envelope(&envelope, None)
            .await
            .map_err(err)?;
        if let Some(error) = &sim.error {
            return Err(format!("simulation failed: {error}"));
        }
        let mut sim_data = sim.transaction_data().map_err(err)?;
        sim_data.resource_fee = i64::try_from(sim.min_resource_fee).unwrap_or(i64::MAX);
        let fee = (sim.min_resource_fee + 100).min(u32::MAX as u64) as u32;

        let final_tx =
            self.build_extend_transaction(next_seq, current_ledger, extend_to, sim_data, fee)?;

        let payload = signature_payload(&final_tx, &self.network_passphrase)?;
        let signature: Signature = self.signing_key.sign(&payload);
        let verifying: VerifyingKey = self.signing_key.verifying_key();
        let hint = xdr::SignatureHint(verifying.to_bytes()[..4].try_into().map_err(err)?);
        let final_envelope = xdr::TransactionEnvelope::Tx(xdr::TransactionV1Envelope {
            tx: final_tx,
            signatures: vecm(vec![xdr::DecoratedSignature {
                hint,
                signature: xdr::Signature::try_from(signature.to_bytes().to_vec()).map_err(err)?,
            }])?,
        });

        let response = self
            .client
            .send_transaction_polling(&final_envelope)
            .await
            .map_err(err)?;
        if response.status == "SUCCESS" {
            Ok(ExtendOutcome::Submitted {
                tx_hash: response.tx_hash.unwrap_or_else(|| "unknown".to_string()),
            })
        } else {
            Err(format!(
                "extension transaction failed with status `{}`",
                response.status
            ))
        }
    }
}
