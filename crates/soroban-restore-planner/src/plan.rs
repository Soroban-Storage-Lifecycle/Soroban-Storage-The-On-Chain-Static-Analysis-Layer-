//! The pure planning core: build the probe/final `RestoreFootprintOp`
//! transaction and read the archived footprint back out of a simulation.
//!
//! No network access happens here, so this is the part of the planner that is
//! exhaustively unit-tested.

use soroban_stellar_common::{muxed, vecm};
use stellar_xdr as xdr;
use stellar_xdr::{Limits, WriteXdr};

use crate::keys::encode_ledger_key;

/// A ledger entry the planner may need to restore, with a stable label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Human-readable label (e.g. `instance`, `code`, `data:0`).
    pub label: String,
    /// Base64-XDR `LedgerKey`.
    pub key_xdr: String,
}

/// The resource data and fee returned by simulating a restore transaction.
#[derive(Clone, Debug)]
pub struct Simulation {
    /// Accurate `SorobanTransactionData` (footprint + resources) for the
    /// entries that actually require restoration.
    pub transaction_data: xdr::SorobanTransactionData,
    /// Simulation-reported resource fee, in stroops.
    pub min_resource_fee: u64,
}

/// The result of planning one contract's restoration.
#[derive(Clone, Debug)]
pub struct PlannedRestore {
    /// The ledger the plan was built against.
    pub current_ledger: u32,
    /// All candidate entries that were considered.
    pub candidates: Vec<Candidate>,
    /// The subset of candidates that are archived and require restoration.
    pub archived: Vec<Candidate>,
    /// Simulation-reported resource fee for the restore, in stroops.
    pub min_resource_fee: u64,
    /// The ready-to-sign `RestoreFootprintOp` transaction.
    pub transaction: xdr::Transaction,
    /// The unsigned transaction envelope as base64-XDR.
    pub unsigned_xdr: String,
}

/// Builds the candidate footprint for a restore: every candidate goes in the
/// read-write set (restore rewrites the entries' TTLs).
pub fn candidate_footprint(candidates: &[Candidate]) -> Result<xdr::LedgerFootprint, String> {
    let keys = candidates
        .iter()
        .map(|c| crate::keys::decode_ledger_key(&c.key_xdr))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(xdr::LedgerFootprint {
        read_only: vecm(Vec::new())?,
        read_write: vecm(keys)?,
    })
}

/// Builds a `RestoreFootprintOp` transaction carrier for simulation (with
/// placeholder resources) or as the final transaction.
pub fn build_restore_transaction(
    source: &xdr::AccountId,
    seq_num: i64,
    current_ledger: u32,
    resources: xdr::SorobanTransactionData,
    fee: u32,
) -> Result<xdr::Transaction, String> {
    let op = xdr::Operation {
        source_account: None,
        body: xdr::OperationBody::RestoreFootprint(xdr::RestoreFootprintOp {
            ext: xdr::ExtensionPoint::V0,
        }),
    };
    // A generous validity window so the emitted transaction can be signed and
    // submitted out of band.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let preconditions = xdr::Preconditions::V2(xdr::PreconditionsV2 {
        time_bounds: Some(xdr::TimeBounds {
            min_time: xdr::TimePoint(0),
            max_time: xdr::TimePoint(now + 3_600),
        }),
        ledger_bounds: Some(xdr::LedgerBounds {
            min_ledger: current_ledger,
            max_ledger: current_ledger + 1_000,
        }),
        min_seq_num: None,
        min_seq_age: xdr::Duration(0),
        min_seq_ledger_gap: 0,
        extra_signers: vecm(Vec::new())?,
    });
    Ok(xdr::Transaction {
        source_account: muxed(source),
        fee,
        seq_num: xdr::SequenceNumber(seq_num),
        cond: preconditions,
        memo: xdr::Memo::None,
        operations: vecm(vec![op])?,
        ext: xdr::TransactionExt::V1(resources),
    })
}

/// Placeholder resource data carrying just the candidate footprint.
pub fn placeholder_resources(footprint: xdr::LedgerFootprint) -> xdr::SorobanTransactionData {
    xdr::SorobanTransactionData {
        ext: xdr::SorobanTransactionDataExt::V0,
        resources: xdr::SorobanResources {
            footprint,
            instructions: 0,
            disk_read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee: 0,
    }
}

/// The base64-XDR ledger keys in the read-write footprint of a simulation —
/// i.e. the entries the network says must be restored.
pub fn archived_keys(data: &xdr::SorobanTransactionData) -> Result<Vec<String>, String> {
    data.resources
        .footprint
        .read_write
        .iter()
        .map(encode_ledger_key)
        .collect()
}

/// Wraps a transaction in an unsigned V1 envelope and base64-encodes it.
pub fn encode_unsigned(tx: &xdr::Transaction) -> Result<String, String> {
    let envelope = xdr::TransactionEnvelope::Tx(xdr::TransactionV1Envelope {
        tx: tx.clone(),
        signatures: vecm(Vec::new())?,
    });
    envelope
        .to_xdr_base64(Limits::none())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{contract_code_key, contract_instance_key, encode_ledger_key};
    use stellar_xdr::ReadXdr;

    fn account() -> xdr::AccountId {
        xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(
            [5u8; 32],
        )))
    }

    fn candidate(label: &str, key: xdr::LedgerKey) -> Candidate {
        Candidate {
            label: label.to_string(),
            key_xdr: encode_ledger_key(&key).unwrap(),
        }
    }

    #[test]
    fn restore_operation_is_wrapped_correctly() {
        let tx = build_restore_transaction(
            &account(),
            42,
            1000,
            placeholder_resources(candidate_footprint(&[]).unwrap()),
            100,
        )
        .unwrap();
        assert_eq!(tx.seq_num.0, 42);
        assert_eq!(tx.fee, 100);
        assert_eq!(tx.operations.len(), 1);
        match &tx.operations[0].body {
            xdr::OperationBody::RestoreFootprint(op) => {
                assert_eq!(op.ext, xdr::ExtensionPoint::V0);
            }
            other => panic!("expected RestoreFootprint, got {other:?}"),
        }
    }

    #[test]
    fn candidate_footprint_holds_all_keys_in_read_write() {
        let candidates = vec![
            candidate("instance", contract_instance_key([1u8; 32])),
            candidate("code", contract_code_key([2u8; 32])),
        ];
        let footprint = candidate_footprint(&candidates).unwrap();
        assert_eq!(footprint.read_write.len(), 2);
        assert!(footprint.read_only.is_empty());
    }

    #[test]
    fn archived_keys_reads_back_the_read_write_footprint() {
        let candidates = vec![candidate("instance", contract_instance_key([1u8; 32]))];
        let footprint = candidate_footprint(&candidates).unwrap();
        let data = placeholder_resources(footprint);
        assert_eq!(
            archived_keys(&data).unwrap(),
            vec![candidates[0].key_xdr.clone()]
        );
    }

    #[test]
    fn unsigned_envelope_roundtrips() {
        let tx = build_restore_transaction(
            &account(),
            7,
            100,
            placeholder_resources(candidate_footprint(&[]).unwrap()),
            100,
        )
        .unwrap();
        let b64 = encode_unsigned(&tx).unwrap();
        let decoded =
            xdr::TransactionEnvelope::from_xdr_base64(&b64, stellar_xdr::Limits::none()).unwrap();
        let envelope = match decoded {
            xdr::TransactionEnvelope::Tx(e) => e,
            other => panic!("expected V1 envelope, got {other:?}"),
        };
        assert_eq!(envelope.tx.seq_num.0, 7);
        assert!(envelope.signatures.is_empty());
    }

    #[test]
    fn transaction_source_is_the_configured_account() {
        let tx = build_restore_transaction(
            &account(),
            7,
            100,
            placeholder_resources(candidate_footprint(&[]).unwrap()),
            100,
        )
        .unwrap();
        let expected = muxed(&account());
        assert_eq!(tx.source_account, expected);
    }
}
