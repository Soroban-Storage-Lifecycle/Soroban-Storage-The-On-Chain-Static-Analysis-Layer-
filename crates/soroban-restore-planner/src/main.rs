//! `soroban-restore-planner` — plan (and optionally submit) a
//! `RestoreFootprintOp` for an archived Soroban contract.
//!
//! ```text
//! soroban-restore-planner --contract C... --source G... \
//!     --rpc-url https://soroban-testnet.stellar.org \
//!     --network-passphrase "Test SDF Network ; September 2015"
//! ```
//!
//! See `docs/restore-planner.md` for the full reference.

use std::process::ExitCode;

use serde::Serialize;
use soroban_restore_planner::keys::{
    contract_code_key, contract_instance_key, decode_account_id, decode_contract_id,
    decode_ledger_key, encode_ledger_key,
};
use soroban_restore_planner::plan::Candidate;
use soroban_restore_planner::planner::Planner;
use soroban_restore_planner::stellar::{self, StellarRpc};
use soroban_restore_planner::Rpc;

const HELP: &str = "\
soroban-restore-planner — emit a RestoreFootprintOp for archived contract state

USAGE:
    soroban-restore-planner --contract <C...> [OPTIONS]

REQUIRED:
    --contract <C...>            Contract id to restore
    --network-passphrase <PASS>  Network passphrase (or SOROBAN_NETWORK_PASSPHRASE)
    --rpc-url <URL>              Stellar RPC endpoint (or SOROBAN_RPC_URL)
    --source <G...>              Transaction source account (or derived from
                                 --signer-secret)

OPTIONS:
    --signer-secret <S...>       Secret key used to sign with --submit
                                 (or SOROBAN_SIGNER_SECRET)
    --key <BASE64_XDR>           Extra contract-data LedgerKey to consider
                                 (repeatable)
    --wasm-hash <HEX>            Contract code hash (64 hex chars) to include
                                 when the instance entry is archived
    --submit                     Sign (requires --signer-secret) and submit
    --out <PATH>                 Write the unsigned envelope to a file
    --json                       Emit a JSON report
    -h, --help                   Print this help and exit
";

#[derive(Default)]
struct Args {
    contract: Option<String>,
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

fn run(args: &Args) -> Result<Report, String> {
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

    let source = match &signer_secret {
        Some(secret) => stellar::account_from_secret(secret)?,
        None => {
            let source = args.source.as_deref().ok_or("--source is required")?;
            decode_account_id(source)?
        }
    };

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let rpc = StellarRpc::new(&rpc_url)?;
        rpc.verify_network(&passphrase).await?;

        // Candidate entries: the instance entry is always considered; the code
        // entry is included when we can resolve its hash (from `--wasm-hash`,
        // or from the live instance); explicit data keys are added verbatim.
        let mut candidates = vec![Candidate {
            label: "instance".to_string(),
            key_xdr: encode_ledger_key(&contract_instance_key(contract_id))?,
        }];
        let resolved_hash = match &wasm_hash {
            Some(hex) => Some(decode_hex32(hex)?),
            None => rpc.fetch_wasm_hash(contract_id).await?,
        };
        if let Some(hash) = resolved_hash {
            candidates.push(Candidate {
                label: "code".to_string(),
                key_xdr: encode_ledger_key(&contract_code_key(hash))?,
            });
        } else {
            eprintln!(
                "note: could not resolve the contract code hash (instance archived?); \
                 pass --wasm-hash to include the code entry"
            );
        }
        for (i, key) in keys.iter().enumerate() {
            // Validate the user-provided key up front for a clear error.
            decode_ledger_key(key)?;
            candidates.push(Candidate {
                label: format!("data:{i}"),
                key_xdr: key.clone(),
            });
        }

        let planner = Planner::new(rpc, source);
        let planned = planner.plan(candidates).await?;

        let submitted_tx_hash = if submit {
            let secret = signer_secret
                .as_deref()
                .ok_or("--submit requires --signer-secret")?;
            let signed = stellar::sign_envelope(&planned.transaction, secret, &passphrase)?;
            Some(planner.submit(&signed).await?)
        } else {
            None
        };

        Ok(Report {
            contract: contract.clone(),
            current_ledger: planned.current_ledger,
            candidates: planned.candidates.iter().map(EntryReport::from).collect(),
            archived: planned.archived.iter().map(EntryReport::from).collect(),
            min_resource_fee: planned.min_resource_fee,
            unsigned_transaction_xdr: planned.unsigned_xdr,
            submitted_tx_hash,
        })
    })
}

fn print_human(report: &Report) {
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

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };

    match run(&args) {
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
                print_human(&report);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
