//! Static analysis for Soroban contract storage.
//!
//! Flags the storage footguns that cost user funds:
//!
//! - **R1 `temporary_critical_type`** (error): balances / funds / positions
//!   written to `env.storage().temporary()`. Temporary entries are permanently
//!   deleted on expiry — never restorable.
//! - **R1b `lifecycle_mismatch`** (error): a `storage!`-declared key accessed
//!   through the wrong lifecycle (each lifecycle is a separate key space).
//! - **R2 `missing_ttl_extension`** (warning): write paths that never extend
//!   the entry TTL, so entries drift toward eviction.
//! - **R3 `instance_storage_bloat`** (warning): unbounded or loop-written
//!   values in instance storage (a single ledger entry paid on every call).
//! - **R4 `temporary_ttl_exceeded`** (warning): extensions beyond the network
//!   maximum TTL, which the host silently clamps.
//!
//! The analyzer is syntactic (syn-based) on purpose: it runs in milliseconds on
//! source without building the contract, like Slither for Solidity. The
//! `storage!` macro in `soroban-storage` provides the hard compile-time
//! guarantees; these rules cover raw `env.storage()` code and structural smells.
//!
//! See `docs/lint-rules.md` for the full reference.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub mod parse;
pub mod rules;

pub use rules::{RuleFinding, Severity};

/// Options controlling a lint run.
#[derive(Debug, Clone, Default)]
pub struct LintOptions {
    /// Treat warnings as failures (affects exit code, not findings).
    pub deny_warnings: bool,
    /// Rules to skip by name.
    pub ignored_rules: HashSet<String>,
}

/// A lint finding with file context attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub severity: Severity,
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub snippet: String,
}

impl Finding {
    /// Renders one line of human-readable output.
    pub fn render(&self) -> String {
        format!(
            "{}:{}:{}: [{}] {} (rule: {})",
            self.path.display(),
            self.line,
            self.column,
            self.severity.as_str(),
            self.message,
            self.rule
        )
    }
}

/// Lints one source string against the rules.
pub fn lint_source(path: &Path, source: &str, opts: &LintOptions) -> Vec<Finding> {
    let Ok(file) = syn::parse_file(source) else {
        // Unparseable files are the compiler's job, not ours.
        return Vec::new();
    };
    let model = parse::scan_file(&file);
    let mut findings = Vec::new();
    rules::run_all(&model, &opts.ignored_rules, &mut findings);

    let lines: Vec<&str> = source.lines().collect();
    findings
        .into_iter()
        .map(|rf| {
            let snippet = lines
                .get(rf.line.saturating_sub(1))
                .unwrap_or(&"")
                .trim()
                .to_string();
            Finding {
                rule: rf.rule,
                severity: rf.severity,
                path: path.to_path_buf(),
                line: rf.line,
                column: rf.column,
                message: rf.message,
                snippet,
            }
        })
        .collect()
}

/// Reads and lints one `.rs` file.
pub fn lint_file(path: &Path, opts: &LintOptions) -> Result<Vec<Finding>, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(lint_source(path, &source, opts))
}

/// Recursively collects `.rs` files under `root`, skipping build/hidden dirs.
pub fn collect_rs_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rs_files_into(root, &mut out);
    out.sort();
    out
}

fn collect_rs_files_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules") {
                continue;
            }
            collect_rs_files_into(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
