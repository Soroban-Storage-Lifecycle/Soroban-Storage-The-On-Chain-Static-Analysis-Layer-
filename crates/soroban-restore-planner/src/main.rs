//! `soroban-restore-planner` — plan (and optionally submit) a
//! `RestoreFootprintOp` for archived Soroban contract state.
//!
//! Single contract:
//!
//! ```text
//! soroban-restore-planner --contract C... --source G... \
//!     --rpc-url https://soroban-testnet.stellar.org \
//!     --network-passphrase "Test SDF Network ; September 2015"
//! ```
//!
//! Batch mode restores every contract listed in a config file:
//!
//! ```text
//! soroban-restore-planner --config contracts.json
//! ```
//!
//! See `docs/restore-planner.md` for the full reference.

use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use soroban_restore_planner::keys::{decode_account_id, decode_contract_id};
use soroban_restore_planner::plan::Candidate;
use soroban_restore_planner::planner::Planner;
use soroban_restore_planner::stellar::{self, StellarRpc};
use soroban_restore_planner::Rpc;
use stellar_xdr::AccountId;

const HELP: &str = "\
soroban-restore-planner — emit a RestoreFootprintOp for archived contract state

USAGE:
    soroban-restore-planner --contract <C...> [OPTIONS]
    soroban-restore-planner --config <file.json> [OPTIONS]

REQUIRED (single contract mode):
    --contract <C...>            Contract id to restore
    --network-passphrase <PASS>  Network passphrase (or SOROBAN_NETWORK_PASSPHRASE)
    --rpc-url <URL>              Stellar RPC endpoint (or SOROBAN_RPC_URL)
    --source <G...>              Transaction source account, or derive it from
                                 --signer-secret

BATCH MODE:
    --config <file.json>         Restore every contract in the config file. The
                                 file carries rpc_url, network_passphrase,
                                 source/signer_secret, submit and contracts[].
                                 Flags below override the file when present.

OPTIONS:
    --signer-secret <S...>       Secret key used to sign with --submit
                                 (or SOROBAN_SIGNER_SECRET)
    --key <BASE64_XDR>           Extra contract-data LedgerKey to consider
                                 (repeatable; single contract mode)
    --wasm-hash <HEX>            Contract code hash (64 hex chars) to include
                                 when the instance entry is archived
    --submit                     Sign (requires --signer-secret) and submit
    --out <PATH>                 File (single) or directory (batch) for the
                                 unsigned envelope(s)
    --json                       Emit a JSON report
    -h, --help                   Print this help and exit

In batch mode the run continues past failures and exits non-zero if any
contract failed.
";

#[derive(Default)]
struct Args {
    contract: Option<String>,
    config: Option<String>,
    rpc_url: Option<String>,
    network_passphrase: Option<String>,
    source: Option<String>,
    signer_secret: Option<String>,
    keys: Vec<String>,
    wasm_hash: Option<String>,
    submit: bool,
    out: Option<String>,
    json: bool,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].clone();
        let value_of = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("missing value for `{arg}`"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--contract" => args.contract = Some(value_of(&mut i)?),
            "--config" => args.config = Some(value_of(&mut i)?),
            "--rpc-url" => args.rpc_url = Some(value_of(&mut i)?),
            "--network-passphrase" => args.network_passphrase = Some(value_of(&mut i)?),
            "--source" => args.source = Some(value_of(&mut i)?),
            "--signer-secret" => args.signer_secret = Some(value_of(&mut i)?),
            "--key" => args.keys.push(value_of(&mut i)?),
            "--wasm-hash" => args.wasm_hash = Some(value_of(&mut i)?),
            "--out" => args.out = Some(value_of(&mut i)?),
            "--submit" => args.submit = true,
            "--json" => args.json = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
        i += 1;
    }
    if let Ok(v) = std::env::var("SOROBAN_RPC_URL") {
        args.rpc_url.get_or_insert(v);
    }
    if let Ok(v) = std::env::var("SOROBAN_NETWORK_PASSPHRASE") {
        args.network_passphrase.get_or_insert(v);
    }
    if let Ok(v) = std::env::var("SOROBAN_SIGNER_SECRET") {
        args.signer_secret.get_or_insert(v);
    }
    Ok(args)
}

fn decode_hex32(input: &str) -> Result<[u8; 32], String> {
    if input.len() != 64 {
        return Err(format!("expected 64 hex characters, got {}", input.len()));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&input[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("invalid hex in wasm hash: {e}"))?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct EntryReport {
    label: String,
    key_xdr: String,
}

impl From<&Candidate> for EntryReport {
    fn from(c: &Candidate) -> Self {
        EntryReport {
            label: c.label.clone(),
            key_xdr: c.key_xdr.clone(),
        }
    }
}

#[derive(Serialize)]
struct Report {
    contract: String,
    current_ledger: u32,
    candidates: Vec<EntryReport>,
    archived: Vec<EntryReport>,
    min_resource_fee: u64,
    unsigned_transaction_xdr: String,
    submitted_tx_hash: Option<String>,
}

#[derive(Serialize)]
struct BatchResult {
    contract: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<Report>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// Batch config
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct BatchConfig {
    rpc_url: String,
    network_passphrase: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    signer_secret: Option<String>,
    #[serde(default)]
    submit: bool,
    contracts: Vec<BatchContract>,
}

#[derive(Deserialize)]
struct BatchContract {
    contract_id: String,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    wasm_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared planning
// ---------------------------------------------------------------------------

fn note_if_unresolved_code(contract: &str, candidates: &[Candidate]) {
    if !candidates.iter().any(|c| c.label == "code") {
        eprintln!(
            "note: {contract}: could not resolve the contract code hash \
             (instance archived?); pass a wasm_hash to include the code entry"
        );
    }
}

async fn plan_contract<R: Rpc>(
    planner: &Planner<R>,
    contract: &str,
    candidates: Vec<Candidate>,
    signer_secret: Option<&str>,
    passphrase: &str,
    submit: bool,
) -> Result<Report, String> {
    let planned = planner.plan(candidates).await?;
    let submitted_tx_hash = if submit {
        let secret = signer_secret.ok_or("--submit requires --signer-secret")?;
        let signed = stellar::sign_envelope(&planned.transaction, secret, passphrase)?;
        Some(planner.submit(&signed).await?)
    } else {
        None
    };
    Ok(Report {
        contract: contract.to_string(),
        current_ledger: planned.current_ledger,
        candidates: planned.candidates.iter().map(EntryReport::from).collect(),
        archived: planned.archived.iter().map(EntryReport::from).collect(),
        min_resource_fee: planned.min_resource_fee,
        unsigned_transaction_xdr: planned.unsigned_xdr,
        submitted_tx_hash,
    })
}

// ---------------------------------------------------------------------------
// Single contract mode
// ---------------------------------------------------------------------------

fn run_single(args: &Args) -> Result<Report, String> {
    let contract = args.contract.clone().ok_or("--contract is required")?;
    let rpc_url = args.rpc_url.clone().ok_or("--rpc-url is required")?;
    let passphrase = args
        .network_passphrase
        .clone()
        .ok_or("--network-passphrase is required")?;
    let signer_secret = args.signer_secret.clone();
    let submit = args.submit;
    let keys = args.keys.clone();
    let wasm_hash = args.wasm_hash.clone();

    let contract_id = decode_contract_id(&contract)?;
    let source = resolve_source(signer_secret.as_deref(), args.source.as_deref())?;

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let rpc = StellarRpc::new(&rpc_url)?;
        rpc.verify_network(&passphrase).await?;
        let planner = Planner::new(rpc, source);

        let wasm = match &wasm_hash {
            Some(hex) => Some(decode_hex32(hex)?),
            None => None,
        };
        let candidates = planner.candidates(contract_id, &keys, wasm).await?;
        note_if_unresolved_code(&contract, &candidates);
        plan_contract(
            &planner,
            &contract,
            candidates,
            signer_secret.as_deref(),
            &passphrase,
            submit,
        )
        .await
    })
}

// ---------------------------------------------------------------------------
// Batch mode
// ---------------------------------------------------------------------------

fn run_batch(args: &Args) -> Result<Vec<BatchResult>, String> {
    let path = args.config.clone().ok_or("--config is required")?;
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("cannot read config {path}: {e}"))?;
    let config: BatchConfig =
        serde_json::from_str(&raw).map_err(|e| format!("invalid config {path}: {e}"))?;
    if config.contracts.is_empty() {
        return Err(format!("config {path} lists no contracts"));
    }

    // Flags override the file when provided.
    let rpc_url = args.rpc_url.clone().unwrap_or(config.rpc_url);
    let passphrase = args
        .network_passphrase
        .clone()
        .unwrap_or(config.network_passphrase);
    let signer_secret = args.signer_secret.clone().or(config.signer_secret);
    let submit = args.submit || config.submit;
    let source = resolve_source(
        signer_secret.as_deref(),
        args.source.as_deref().or(config.source.as_deref()),
    )?;

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let rpc = StellarRpc::new(&rpc_url)?;
        rpc.verify_network(&passphrase).await?;
        let planner = Planner::new(rpc, source);

        let mut results = Vec::new();
        for contract in &config.contracts {
            let outcome = async {
                let id = decode_contract_id(&contract.contract_id)?;
                let wasm = match &contract.wasm_hash {
                    Some(hex) => Some(decode_hex32(hex)?),
                    None => None,
                };
                let candidates = planner.candidates(id, &contract.keys, wasm).await?;
                note_if_unresolved_code(&contract.contract_id, &candidates);
                plan_contract(
                    &planner,
                    &contract.contract_id,
                    candidates,
                    signer_secret.as_deref(),
                    &passphrase,
                    submit,
                )
                .await
            }
            .await;

            results.push(match outcome {
                Ok(report) => BatchResult {
                    contract: contract.contract_id.clone(),
                    ok: true,
                    report: Some(report),
                    error: None,
                },
                Err(error) => BatchResult {
                    contract: contract.contract_id.clone(),
                    ok: false,
                    report: None,
                    error: Some(error),
                },
            });
        }
        Ok(results)
    })
}

/// Resolves the transaction source: the signer secret wins when present.
fn resolve_source(signer_secret: Option<&str>, source: Option<&str>) -> Result<AccountId, String> {
    match signer_secret {
        Some(secret) => stellar::account_from_secret(secret),
        None => {
            let source = source.ok_or("--source (or config source) is required")?;
            decode_account_id(source)
        }
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_report(report: &Report) {
    println!("contract:        {}", report.contract);
    println!("current ledger:  {}", report.current_ledger);
    println!("candidates:      {}", report.candidates.len());
    println!("archived:        {}", report.archived.len());
    for entry in &report.archived {
        println!("  - {} ({})", entry.label, entry.key_xdr);
    }
    println!("resource fee:    {} stroops", report.min_resource_fee);
    if let Some(hash) = &report.submitted_tx_hash {
        println!("submitted:       {hash}");
    } else {
        println!("unsigned transaction (base64 XDR):");
        println!("{}", report.unsigned_transaction_xdr);
    }
}

fn write_batch_envelopes(dir: &str, results: &[BatchResult]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
    for result in results {
        if let Some(report) = &result.report {
            if report.submitted_tx_hash.is_some() {
                continue;
            }
            let path = std::path::Path::new(dir).join(format!("{}.xdr", result.contract));
            std::fs::write(&path, &report.unsigned_transaction_xdr)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };

    if args.config.is_some() {
        return run_batch_main(&args);
    }

    match run_single(&args) {
        Ok(report) => {
            if let Some(out) = &args.out {
                if let Err(e) = std::fs::write(out, &report.unsigned_transaction_xdr) {
                    eprintln!("error: cannot write {out}: {e}");
                    return ExitCode::FAILURE;
                }
                eprintln!("wrote unsigned envelope to {out}");
            }
            if args.json {
                match serde_json::to_string_pretty(&report) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("error: cannot serialize report: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                print_report(&report);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_batch_main(args: &Args) -> ExitCode {
    let results = match run_batch(args) {
        Ok(results) => results,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(out) = &args.out {
        if let Err(e) = write_batch_envelopes(out, &results) {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    }

    if args.json {
        match serde_json::to_string_pretty(&results) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("error: cannot serialize report: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        for result in &results {
            match (&result.report, &result.error) {
                (Some(report), _) => print_report(report),
                (None, Some(error)) => {
                    eprintln!("contract:        {}", result.contract);
                    eprintln!("error:           {error}");
                }
                (None, None) => {}
            }
            println!();
        }
        let failed = results.iter().filter(|r| !r.ok).count();
        println!(
            "{} contract(s): {} ok, {} failed",
            results.len(),
            results.len() - failed,
            failed
        );
    }

    if results.iter().any(|r| !r.ok) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
