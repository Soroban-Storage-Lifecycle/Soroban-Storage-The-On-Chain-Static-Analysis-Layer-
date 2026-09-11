//! `cargo soroban-lint` — storage static analysis for Soroban contracts.
//!
//! Installed as `cargo install --path crates/soroban-storage-lints`, the
//! `cargo-soroban-lint` binary is auto-discovered as the `cargo soroban-lint`
//! subcommand.
//!
//! Usage:
//! ```text
//! cargo soroban-lint [paths...] [--deny-warnings] [--json] [--ignore RULE]
//! ```
//!
//! Exit codes: `0` clean (or warnings without `--deny-warnings`), `1` errors
//! found (or warnings with `--deny-warnings`), `2` usage error.

use std::path::PathBuf;
use std::process::ExitCode;

use soroban_storage_lints::{collect_rs_files, lint_file, Finding, LintOptions, Severity};

fn help() -> &'static str {
    "\
cargo soroban-lint — Soroban storage static analysis

USAGE:
    cargo soroban-lint [OPTIONS] [PATHS]...

ARGS:
    <PATHS>...    Directories to scan for .rs files (default: ./src if it
                  exists, otherwise the current directory)

OPTIONS:
    --deny-warnings    Fail the run on warnings as well as errors
    --json             Emit findings as a JSON array
    --ignore <RULE>    Skip a rule by name (repeatable)
    --version          Print version and exit
    -h, --help         Print help and exit

RULES:
    temporary_critical_type   error    critical data in temporary storage
    lifecycle_mismatch        error    storage! key used via wrong lifecycle
    missing_ttl_extension     warning  write path without TTL extension
    instance_storage_bloat    warning  unbounded/looped instance storage
    temporary_ttl_exceeded    warning  temporary extension beyond network max
    persistent_ttl_exceeded   warning  persistent/instance extension beyond max

EXIT CODES:
    0    clean (or warnings only, without --deny-warnings)
    1    lint errors found (or warnings with --deny-warnings)
    2    usage error
"
}

fn parse_args() -> Result<(Vec<PathBuf>, LintOptions, bool), String> {
    let mut paths = Vec::new();
    let mut opts = LintOptions::default();
    let mut json = false;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--deny-warnings" => opts.deny_warnings = true,
            "--json" => json = true,
            "--ignore" => {
                let rule = args.next().ok_or("--ignore requires a rule name")?;
                opts.ignored_rules.insert(rule);
            }
            "--version" => {
                println!("soroban-storage-lints {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-h" | "--help" => {
                print!("{}", help());
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option `{other}` (see --help)"));
            }
            other => paths.push(PathBuf::from(other)),
        }
    }
    if paths.is_empty() {
        let src = PathBuf::from("src");
        paths.push(if src.is_dir() {
            src
        } else {
            PathBuf::from(".")
        });
    }
    Ok((paths, opts, json))
}

fn main() -> ExitCode {
    let (paths, opts, json) = match parse_args() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", help());
            return ExitCode::from(2);
        }
    };

    let mut files = Vec::new();
    for path in &paths {
        if path.is_file() {
            files.push(path.clone());
        } else {
            files.extend(collect_rs_files(path));
        }
    }
    files.sort();
    files.dedup();

    let mut findings: Vec<Finding> = Vec::new();
    for file in &files {
        match lint_file(file, &opts) {
            Ok(mut fs) => findings.append(&mut fs),
            Err(e) => eprintln!("error: {e}"),
        }
    }
    // Stable ordering: by path, then line.
    findings.sort_by(|a, b| {
        (a.path.clone(), a.line, a.column).cmp(&(b.path.clone(), b.line, b.column))
    });

    let github_actions = std::env::var("GITHUB_ACTIONS").is_ok_and(|v| v == "true");

    if json {
        let payload: Vec<serde_json::Value> = findings.iter().map(to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        for f in &findings {
            if github_actions {
                let (verb, kind) = match f.severity {
                    Severity::Error => ("error", "error"),
                    Severity::Warning => ("warning", "warning"),
                };
                println!(
                    "::{verb} file={},line={},col={}::[{}] {} (rule: {})",
                    f.path.display(),
                    f.line,
                    f.column,
                    kind,
                    f.message,
                    f.rule
                );
            } else {
                println!("{}", f.render());
            }
        }
    }

    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warnings = findings
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .count();
    let failed = errors > 0 || (opts.deny_warnings && warnings > 0);

    if !json {
        eprintln!(
            "{} {} in {} file(s): {errors} error(s), {warnings} warning(s)",
            if failed {
                "storage lint failed:"
            } else {
                "storage lint finished:"
            },
            findings.len(),
            files.len()
        );
    }

    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn to_json(f: &Finding) -> serde_json::Value {
    serde_json::json!({
        "rule": f.rule,
        "severity": f.severity.as_str(),
        "path": f.path.display().to_string(),
        "line": f.line,
        "column": f.column,
        "message": f.message,
        "snippet": f.snippet,
    })
}
