//! `soroban-rent-keeper` — watches deployed contract TTLs and extends them
//! before eviction.
//!
//! ```text
//! soroban-rent-keeper --config config.json
//! ```
//!
//! See `docs/rent-keeper.md` for the configuration reference and a sample
//! `config.json`.

use std::process::ExitCode;
use std::time::Duration;

use soroban_rent_keeper::config::Config;
use soroban_rent_keeper::keeper::Keeper;
use soroban_rent_keeper::metrics;
use soroban_rent_keeper::risk;
use soroban_rent_keeper::stellar::StellarRpc;

fn help() -> &'static str {
    "\
soroban-rent-keeper — extend Soroban contract TTLs before eviction

USAGE:
    soroban-rent-keeper --config <config.json>

ENVIRONMENT:
    SOROBAN_RPC_URL         override rpc_url
    SOROBAN_SIGNER_SECRET   override signer_secret

OPTIONS:
    -c, --config <PATH>   Path to the JSON configuration (required)
    -h, --help            Print help and exit
"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config_path = match args.as_slice() {
        [] => {
            print!("{}", help());
            return ExitCode::SUCCESS;
        }
        [flag] if flag == "-h" || flag == "--help" => {
            print!("{}", help());
            return ExitCode::SUCCESS;
        }
        [flag, path] if flag == "-c" || flag == "--config" => path.clone(),
        _ => {
            eprintln!("usage: soroban-rent-keeper --config <config.json>");
            return ExitCode::from(2);
        }
    };

    let config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(2);
        }
    };

    let metrics = metrics::Metrics::new();
    let _metrics_thread = metrics::spawn_server(&config.metrics_addr, metrics.clone());

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("runtime error: {e}");
            return ExitCode::FAILURE;
        }
    };

    rt.block_on(async move {
        let rpc = match StellarRpc::new(
            &config.rpc_url,
            &config.network_passphrase,
            &config.signer_secret,
        ) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("rpc error: {e}");
                return ExitCode::FAILURE;
            }
        };

        if let Err(e) = rpc.verify_network().await {
            eprintln!("network error: {e}");
            return ExitCode::FAILURE;
        }

        // Fail fast if the signer cannot pay for extensions; otherwise the
        // keeper would tick along reporting `error` metrics while entries expire.
        match rpc.signer_balance_stroops().await {
            Ok(balance) => {
                if !risk::signer_balance_is_sufficient(balance, config.min_signer_balance_stroops) {
                    eprintln!(
                        "signer balance {balance} stroops is below the configured floor {}; \
                         fund the account or lower min_signer_balance_stroops",
                        config.min_signer_balance_stroops
                    );
                    return ExitCode::FAILURE;
                }
                eprintln!(
                    "signer balance {balance} stroops (floor {})",
                    config.min_signer_balance_stroops
                );
            }
            Err(e) => {
                eprintln!("balance check failed: {e}");
                return ExitCode::FAILURE;
            }
        }

        let mut watches = Vec::new();
        for contract in &config.contracts {
            match rpc.build_watch(contract).await {
                Ok(watch) => {
                    eprintln!(
                        "watching {}: {} keys, extend below {} ledgers to {} ledgers",
                        watch.label,
                        watch.keys.len(),
                        watch.threshold_ledgers,
                        watch.extend_to_ledgers
                    );
                    watches.push(watch);
                }
                Err(e) => {
                    eprintln!("watch setup failed: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }

        let keeper = Keeper::new(rpc, watches, metrics, config.max_batch_bytes);
        let interval = Duration::from_secs(config.poll_interval_secs);
        eprintln!(
            "rent keeper started (poll interval {}s)",
            interval.as_secs()
        );
        keeper.run(interval).await;
        ExitCode::SUCCESS
    })
}
