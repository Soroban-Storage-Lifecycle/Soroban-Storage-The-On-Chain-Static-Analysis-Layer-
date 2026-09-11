//! Opt-in smoke test against a live Stellar network.
//!
//! Self-skips (and passes) unless `SOROBAN_RPC_URL`,
//! `SOROBAN_NETWORK_PASSPHRASE`, `SOROBAN_SIGNER_SECRET` and
//! `SOROBAN_TEST_CONTRACT` are all set, so CI stays hermetic.
//!
//! This exercises the **read/simulate** path only: it resolves candidates and
//! builds a plan. Submitting a restore requires a genuinely archived entry to
//! target, which a generic smoke test cannot conjure — see
//! `docs/restore-planner.md` for the manual restore procedure.

use soroban_restore_planner::keys::decode_contract_id;
use soroban_restore_planner::planner::Planner;
use soroban_restore_planner::stellar::{self, StellarRpc};

#[tokio::test]
async fn testnet_plan_smoke() {
    let (Ok(rpc_url), Ok(passphrase), Ok(secret), Ok(contract)) = (
        std::env::var("SOROBAN_RPC_URL"),
        std::env::var("SOROBAN_NETWORK_PASSPHRASE"),
        std::env::var("SOROBAN_SIGNER_SECRET"),
        std::env::var("SOROBAN_TEST_CONTRACT"),
    ) else {
        eprintln!("skipping testnet_plan_smoke: testnet env vars not set");
        return;
    };

    let source = stellar::account_from_secret(&secret).expect("signer secret");
    let rpc = StellarRpc::new(&rpc_url).expect("rpc client");
    rpc.verify_network(&passphrase)
        .await
        .expect("network matches");
    let planner = Planner::new(rpc, source);

    let id = decode_contract_id(&contract).expect("contract id");
    let candidates = planner.candidates(id, &[], None).await.expect("candidates");
    let planned = planner.plan(candidates).await.expect("plan");

    assert!(
        !planned.candidates.is_empty(),
        "expected at least the instance key"
    );
    assert!(!planned.unsigned_xdr.is_empty(), "expected an envelope");
    eprintln!(
        "contract {}: {} candidate(s), {} archived, fee {} stroops, ledger {}",
        contract,
        planned.candidates.len(),
        planned.archived.len(),
        planned.min_resource_fee,
        planned.current_ledger
    );
}
