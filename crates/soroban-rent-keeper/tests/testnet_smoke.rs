//! Opt-in smoke tests against a live Stellar network.
//!
//! These self-skip (and pass) unless the environment provides:
//!
//! - `SOROBAN_RPC_URL` — RPC endpoint, e.g. `https://soroban-testnet.stellar.org`
//! - `SOROBAN_NETWORK_PASSPHRASE` — must match the endpoint
//! - `SOROBAN_SIGNER_SECRET` — funded `S...` account
//! - `SOROBAN_TEST_CONTRACT` — a deployed `C...` contract to watch
//!
//! The write path additionally requires `SOROBAN_TESTNET_WRITE=1`, so no funds
//! are spent merely by running the suite. CI stays hermetic because the vars
//! are absent there.

use soroban_rent_keeper::config::ContractConfig;
use soroban_rent_keeper::keeper::Keeper;
use soroban_rent_keeper::metrics::Metrics;
use soroban_rent_keeper::rpc::Rpc;
use soroban_rent_keeper::stellar::StellarRpc;

struct TestnetEnv {
    rpc_url: String,
    passphrase: String,
    secret: String,
    contract: String,
}

fn testnet_env() -> Option<TestnetEnv> {
    Some(TestnetEnv {
        rpc_url: std::env::var("SOROBAN_RPC_URL").ok()?,
        passphrase: std::env::var("SOROBAN_NETWORK_PASSPHRASE").ok()?,
        secret: std::env::var("SOROBAN_SIGNER_SECRET").ok()?,
        contract: std::env::var("SOROBAN_TEST_CONTRACT").ok()?,
    })
}

fn write_path_enabled() -> bool {
    std::env::var("SOROBAN_TESTNET_WRITE").ok().as_deref() == Some("1")
}

fn contract_config(
    env: &TestnetEnv,
    threshold_ledgers: u32,
    extend_to_ledgers: u32,
) -> ContractConfig {
    ContractConfig {
        contract_id: env.contract.clone(),
        threshold_ledgers,
        extend_to_ledgers,
        data_keys: Vec::new(),
    }
}

/// Read-only: verify the network, resolve the watch keys, and read TTLs.
#[tokio::test]
async fn testnet_read_smoke() {
    let Some(env) = testnet_env() else {
        eprintln!("skipping testnet_read_smoke: testnet env vars not set");
        return;
    };
    let rpc = StellarRpc::new(&env.rpc_url, &env.passphrase, &env.secret).expect("rpc client");
    rpc.verify_network().await.expect("network matches");

    let watch = rpc
        .build_watch(&contract_config(&env, 172_800, 518_400))
        .await
        .expect("watch keys resolve");
    assert!(watch.keys.len() >= 2, "expected instance + code keys");

    let ledger = rpc.latest_ledger().await.expect("latest ledger");
    let entries = rpc.fetch_entries(&watch.keys).await.expect("entries");
    for entry in &entries {
        eprintln!(
            "{}: live_until={} (ttl≈{}) size={}B",
            entry.label,
            entry.live_until_ledger,
            entry.live_until_ledger.saturating_sub(ledger),
            entry.size_bytes
        );
    }
    assert!(
        !entries.is_empty(),
        "expected at least the instance entry to be live"
    );
}

/// Write path: force one `ExtendFootprintTTLOp` and submit it.
#[tokio::test]
async fn testnet_extend_smoke() {
    let Some(env) = testnet_env() else {
        eprintln!("skipping testnet_extend_smoke: testnet env vars not set");
        return;
    };
    if !write_path_enabled() {
        eprintln!(
            "skipping testnet_extend_smoke: set SOROBAN_TESTNET_WRITE=1 to spend testnet XLM"
        );
        return;
    }
    let rpc = StellarRpc::new(&env.rpc_url, &env.passphrase, &env.secret).expect("rpc client");
    // A threshold above any real TTL makes every fetched entry Critical, so one
    // poll submits an extension. This is the only path that spends fees.
    let watch = rpc
        .build_watch(&contract_config(&env, u32::MAX - 1, u32::MAX))
        .await
        .expect("watch keys resolve");
    let keeper = Keeper::new(rpc, vec![watch], Metrics::new(), 200_000);
    let reports = keeper.poll_once().await;
    let report = reports[0].as_ref().expect("poll succeeds");
    assert!(
        report.submitted >= 1,
        "expected at least one ExtendFootprintTTLOp submission, got {report:?}"
    );
}
