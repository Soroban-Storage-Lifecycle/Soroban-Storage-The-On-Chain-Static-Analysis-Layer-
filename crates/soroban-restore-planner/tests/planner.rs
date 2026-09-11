//! Integration tests for the planner flow, driven by a fake [`Rpc`] so no
//! network is required.

use std::sync::Mutex;

use soroban_restore_planner::keys::{contract_code_key, contract_instance_key, encode_ledger_key};
use soroban_restore_planner::plan::{candidate_footprint, Candidate, Simulation};
use soroban_restore_planner::planner::Planner;
use soroban_restore_planner::rpc::Rpc;
use stellar_xdr as xdr;

/// A fake network: everything is canned, submissions are recorded.
struct FakeRpc {
    ledger: u32,
    sequence: i64,
    /// The keys the fake network reports as archived (returned in the
    /// simulation's read-write footprint).
    archived: Vec<Candidate>,
    wasm_hash: Option<[u8; 32]>,
    submitted: Mutex<Vec<String>>,
}

impl FakeRpc {
    fn new(archived: Vec<Candidate>) -> Self {
        FakeRpc {
            ledger: 1_000,
            sequence: 7,
            archived,
            wasm_hash: Some([2u8; 32]),
            submitted: Mutex::new(Vec::new()),
        }
    }
}

impl Rpc for FakeRpc {
    async fn latest_ledger(&self) -> Result<u32, String> {
        Ok(self.ledger)
    }

    async fn account_sequence(&self, _account: &str) -> Result<i64, String> {
        Ok(self.sequence)
    }

    async fn simulate_restore(&self, _probe: &xdr::Transaction) -> Result<Simulation, String> {
        // The fake reports only the archived subset in the read-write
        // footprint, exactly as the real `restorePreamble` does.
        let footprint = candidate_footprint(&self.archived)?;
        let transaction_data = xdr::SorobanTransactionData {
            ext: xdr::SorobanTransactionDataExt::V0,
            resources: xdr::SorobanResources {
                footprint,
                instructions: 0,
                disk_read_bytes: 0,
                write_bytes: 0,
            },
            resource_fee: 0,
        };
        Ok(Simulation {
            transaction_data,
            min_resource_fee: 5_000,
        })
    }

    async fn submit_envelope(&self, envelope_base64: &str) -> Result<String, String> {
        self.submitted
            .lock()
            .unwrap()
            .push(envelope_base64.to_string());
        Ok("deadbeef".to_string())
    }

    async fn fetch_wasm_hash(&self, _contract_id: [u8; 32]) -> Result<Option<[u8; 32]>, String> {
        Ok(self.wasm_hash)
    }
}

fn account() -> xdr::AccountId {
    xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(
        [9u8; 32],
    )))
}

fn candidate(label: &str, key: xdr::LedgerKey) -> Candidate {
    Candidate {
        label: label.to_string(),
        key_xdr: encode_ledger_key(&key).unwrap(),
    }
}

fn candidates() -> Vec<Candidate> {
    vec![
        candidate("instance", contract_instance_key([1u8; 32])),
        candidate("code", contract_code_key([2u8; 32])),
    ]
}

#[tokio::test]
async fn reports_only_the_archived_subset() {
    let all = candidates();
    let archived = vec![all[1].clone()]; // only the code entry is archived
    let planner = Planner::new(FakeRpc::new(archived), account());

    let planned = planner.plan(all.clone()).await.unwrap();

    assert_eq!(planned.candidates.len(), 2);
    assert_eq!(planned.archived.len(), 1);
    assert_eq!(planned.archived[0].label, "code");
    assert_eq!(planned.current_ledger, 1_000);
    assert_eq!(planned.min_resource_fee, 5_000);
    // The final transaction carries the simulated resource fee plus the
    // inclusion fee, and its footprint is the archived subset.
    assert_eq!(planned.transaction.fee, 5_100);
    match &planned.transaction.ext {
        xdr::TransactionExt::V1(data) => {
            assert_eq!(data.resource_fee, 5_000);
            assert_eq!(data.resources.footprint.read_write.len(), 1);
        }
        other => panic!("expected Soroban data, got {other:?}"),
    }
    assert!(!planned.unsigned_xdr.is_empty());
}

#[tokio::test]
async fn nothing_archived_still_produces_a_plan() {
    let planner = Planner::new(FakeRpc::new(Vec::new()), account());
    let planned = planner.plan(candidates()).await.unwrap();
    assert!(planned.archived.is_empty());
    assert_eq!(planned.candidates.len(), 2);
    assert!(!planned.unsigned_xdr.is_empty());
}

#[tokio::test]
async fn submit_delegates_to_the_network() {
    let planner = Planner::new(FakeRpc::new(Vec::new()), account());
    let hash = planner.submit("AAAA").await.unwrap();
    assert_eq!(hash, "deadbeef");
}

/// Sanity check that the footprint helper places every candidate in the
/// read-write set.
#[test]
fn footprint_places_candidates_in_read_write() {
    let footprint = candidate_footprint(&candidates()).unwrap();
    assert_eq!(footprint.read_write.len(), 2);
    assert!(footprint.read_only.is_empty());
}
