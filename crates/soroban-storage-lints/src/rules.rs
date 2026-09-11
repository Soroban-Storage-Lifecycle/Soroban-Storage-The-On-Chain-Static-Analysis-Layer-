//! The storage lint rules.
//!
//! Every rule is written conservatively: when a fact cannot be determined from
//! syntax alone, the rule stays quiet rather than risk a false positive. The
//! `storage!` macro itself provides the hard compile-time guarantees; these
//! rules cover raw `env.storage()` code and structural smells.
//!
//! See `docs/lint-rules.md` for the full rule reference.

use quote::ToTokens;
use syn::Expr;

use crate::parse::{FileModel, FnStorage, Lifecycle, StorageChain};

/// Maximum TTL a temporary entry can be extended to (network parameter, ~180
/// days at ~5s ledgers). Kept local so the linter stays dependency-free.
pub const MAX_TEMP_TTL: u32 = 3_110_400;

/// Instance structs with more fields than this are flagged when written to
/// instance storage.
pub const MAX_INSTANCE_STRUCT_FIELDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// A finding produced by a rule (path/snippet attached by the runner).
#[derive(Debug, Clone)]
pub struct RuleFinding {
    pub rule: &'static str,
    pub severity: Severity,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Value-type resolution (best effort, syntax only)
// ---------------------------------------------------------------------------

/// Words that strongly suggest irreplaceable value when they appear in a type
/// name (camel-case aware).
const CRITICAL_HINTS: [&str; 18] = [
    "balance",
    "amount",
    "fund",
    "collateral",
    "position",
    "ledger",
    "escrow",
    "vault",
    "reserve",
    "deposit",
    "allowance",
    "debt",
    "credit",
    "asset",
    "token",
    "share",
    "reward",
    "claim",
];

fn split_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if !current.is_empty() && (ch.is_uppercase() || ch.is_numeric()) {
                let prev = current.chars().last().unwrap_or_default();
                if prev.is_lowercase() || prev.is_numeric() {
                    words.push(current.clone());
                    current.clear();
                }
            }
            current.push(ch);
        } else if !current.is_empty() {
            words.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words.into_iter().map(|w| w.to_lowercase()).collect()
}

/// Whether a type name suggests critical, irreplaceable data.
pub fn type_looks_critical(type_str: &str) -> bool {
    let words = split_words(type_str);
    words.iter().any(|w| CRITICAL_HINTS.contains(&w.as_str()))
}

/// Whether a type name suggests an unbounded collection / large payload.
pub fn type_looks_collection(type_str: &str) -> bool {
    let words = split_words(type_str);
    words.iter().any(|w| {
        matches!(
            w.as_str(),
            "vec" | "map" | "string" | "bytes" | "set" | "array"
        )
    })
}

/// Best-effort resolution of the value type written through a storage call.
///
/// Returns `None` when the type cannot be determined from syntax alone.
fn resolve_value_type(chain: &StorageChain, f: &FnStorage, model: &FileModel) -> Option<String> {
    let value_expr = match chain.method.as_str() {
        "set" | "try_update" => chain.args.get(1),
        "update" => chain.args.get(1),
        _ => return None,
    }?;
    match value_expr {
        // `set(&key, &val)` — the value is often behind a reference.
        Expr::Reference(r) => resolve_value_expr(&r.expr, f, model),
        Expr::Closure(c) => {
            // `update(&key, |bal: Balance| ...)` — type from the closure param.
            for input in &c.inputs {
                if let syn::Pat::Type(pat) = input {
                    return Some(crate::parse::type_string(&pat.ty));
                }
            }
            None
        }
        other => resolve_value_expr(other, f, model),
    }
}

fn resolve_value_expr(expr: &Expr, f: &FnStorage, model: &FileModel) -> Option<String> {
    match expr {
        // `balance` / `self` / `contract.balance`
        Expr::Path(p) => {
            let name = p.path.segments.last()?.ident.to_string();
            f.bindings.get(&name).cloned().or_else(|| {
                // `self` inside an impl method resolves to the impl's Self type.
                if name == "self" {
                    f.self_type.clone()
                } else {
                    None
                }
            })
        }
        // `self.balance` / `vault.balance` — resolve the field's declared type
        // from the struct definition when the receiver type is known.
        Expr::Field(field) => {
            let base = field.base.as_ref();
            let receiver = match base {
                Expr::Path(p) => {
                    let name = p.path.segments.last()?.ident.to_string();
                    if name == "self" {
                        f.self_type.clone()
                    } else {
                        f.bindings.get(&name).cloned()
                    }
                }
                _ => None,
            }?;
            let field_name = field.member.to_token_stream().to_string().replace(' ', "");
            let fields = model.struct_field_types.get(&receiver)?;
            fields.get(&field_name).cloned()
        }
        // `TokenBalance { amount: 5 }` — the struct literal's type is known
        // from syntax alone.
        Expr::Struct(s) => Some(s.path.segments.last()?.ident.to_string()),
        // `balances.get(user)` / `prices.last()` — a method call on a typed
        // binding; extract the value/element generic argument.
        Expr::MethodCall(mc) => {
            if let Some(ty) = binding_type_of(&mc.receiver, f) {
                if let Some(element) = generic_element(&ty) {
                    return Some(element);
                }
            }
            // `get_balance(&env)` — method name as a hint when it contains a
            // critical word; otherwise unresolved.
            let method = mc.method.to_string();
            if type_looks_critical(&method) {
                Some(method)
            } else {
                None
            }
        }
        // `balances[0]` — index into a typed collection.
        Expr::Index(idx) => {
            let ty = binding_type_of(&idx.expr, f)?;
            generic_element(&ty)
        }
        _ => None,
    }
}

/// The type of the expression a method is called on, when it is a simple
/// binding or `self` field.
fn binding_type_of(expr: &Expr, f: &FnStorage) -> Option<String> {
    match expr {
        Expr::Path(p) => {
            let name = p.path.segments.last()?.ident.to_string();
            if name == "self" {
                f.self_type.clone()
            } else {
                f.bindings.get(&name).cloned()
            }
        }
        _ => None,
    }
}

/// Extracts the value/element type of a generic collection, e.g.
/// `Map<Address, Balance>` -> `Balance`, `Vec<Balance>` -> `Balance`,
/// `Option<Balance>` -> `Balance`. Returns `None` for non-generic types.
fn generic_element(ty: &str) -> Option<String> {
    let open = ty.find('<')?;
    let close = ty.rfind('>')?;
    let args = &ty[open + 1..close];
    let last = args.rsplit(',').next()?;
    let last = last.trim();
    if last.is_empty() || last.contains('<') {
        None
    } else {
        Some(last.to_string())
    }
}

/// Extracts the last path segment of a key expression, e.g.
/// `&StorageKey::Balance(user)` -> `Some("Balance")`.
fn key_variant(chain: &StorageChain) -> Option<String> {
    if chain.lifecycle == Lifecycle::Instance {
        return None;
    }
    let key_expr = chain.args.first()?;
    let mut expr = key_expr;
    // Unwrap references: `&key`.
    while let Expr::Reference(r) = expr {
        expr = &r.expr;
    }
    // Unwrap calls: `StorageKey::Balance(user)`.
    while let Expr::Call(c) = expr {
        expr = &c.func;
    }
    if let Expr::Path(p) = expr {
        return p.path.segments.last().map(|s| s.ident.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// R1 — `temporary_critical_type` (error)
///
/// Critical, irreplaceable data (balances, funds, positions) must never be
/// written to temporary storage: when the TTL expires the entry is permanently
/// deleted and can never be restored.
pub fn temporary_critical_type(model: &FileModel, out: &mut Vec<RuleFinding>) {
    for f in &model.fns {
        for chain in &f.chains {
            if chain.lifecycle != Lifecycle::Temporary
                || !matches!(chain.method.as_str(), "set" | "update" | "try_update")
            {
                continue;
            }
            let mut reasons = Vec::new();
            // 1. Key belongs to a `storage!` entry declared critical.
            if let Some(variant) = key_variant(chain) {
                if let Some(decl) = model.decls.iter().find(|d| d.variant == variant) {
                    if decl.critical {
                        reasons.push(format!(
                            "key `{variant}` is declared `critical` in the storage schema"
                        ));
                    }
                }
            }
            // 2. Value type is `Critical`-derived in this crate.
            if let Some(ty) = resolve_value_type(chain, f, model) {
                if model.critical_types.contains(&ty) {
                    reasons.push(format!("value type `{ty}` derives `Critical`"));
                } else if type_looks_critical(&ty) {
                    reasons.push(format!(
                        "value type `{ty}` matches the critical-data naming heuristic"
                    ));
                }
            }
            if !reasons.is_empty() {
                out.push(RuleFinding {
                    rule: "temporary_critical_type",
                    severity: Severity::Error,
                    line: chain.line,
                    column: chain.column,
                    message: format!(
                        "temporary storage write with critical data ({}): temporary entries are \
                         permanently deleted when their TTL expires and can never be restored. \
                         Use persistent storage (or the `storage!` macro, which rejects this at \
                         compile time).",
                        reasons.join("; ")
                    ),
                });
            }
        }
    }
}

/// R1b — `lifecycle_mismatch` (error)
///
/// A storage key declared with one lifecycle is accessed through a different
/// lifecycle. Each lifecycle is a separate key space, so this silently reads or
/// writes the wrong entry.
pub fn lifecycle_mismatch(model: &FileModel, out: &mut Vec<RuleFinding>) {
    for f in &model.fns {
        for chain in &f.chains {
            let Some(variant) = key_variant(chain) else {
                continue;
            };
            let Some(decl) = model.decls.iter().find(|d| d.variant == variant) else {
                continue;
            };
            if decl.lifecycle != chain.lifecycle {
                out.push(RuleFinding {
                    rule: "lifecycle_mismatch",
                    severity: Severity::Error,
                    line: chain.line,
                    column: chain.column,
                    message: format!(
                        "entry `{variant}` is declared `{}` in the storage schema but accessed \
                         through `.{}()` — lifecycles are separate key spaces, so this reads or \
                         writes the wrong entry",
                        decl.lifecycle.as_str(),
                        chain.lifecycle.as_str()
                    ),
                });
            }
        }
    }
}

/// R2 — `missing_ttl_extension` (warning)
///
/// A function that writes persistent/temporary entries through raw
/// `env.storage()` calls without any TTL management in the same function.
/// Entries whose TTL is never extended eventually expire (temporary: permanent
/// loss; persistent: archive + restore).
pub fn missing_ttl_extension(model: &FileModel, out: &mut Vec<RuleFinding>) {
    for f in &model.fns {
        let writes: Vec<&StorageChain> = f
            .chains
            .iter()
            .filter(|c| {
                c.lifecycle != Lifecycle::Instance
                    && matches!(c.method.as_str(), "set" | "update" | "try_update")
            })
            .collect();
        if writes.is_empty() || f.has_ttl_call {
            continue;
        }
        for chain in writes {
            out.push(RuleFinding {
                rule: "missing_ttl_extension",
                severity: Severity::Warning,
                line: chain.line,
                column: chain.column,
                message: format!(
                    "`{}()` write to {} storage with no TTL extension in this function: the entry \
                     TTL is never bumped here, so it drifts toward expiry. Call `extend_ttl`/`bump` \
                     in this function or use soroban-storage accessors, which auto-bump on access",
                    chain.method,
                    chain.lifecycle.as_str()
                ),
            });
        }
    }
}

/// R3 — `instance_storage_bloat` (warning)
///
/// Instance storage lives inside the single contract-instance entry: everything
/// written there is paid for on every invocation and cannot be pruned. Large
/// values, or writes in loops, bloat the instance and the per-call footprint.
pub fn instance_storage_bloat(model: &FileModel, out: &mut Vec<RuleFinding>) {
    for f in &model.fns {
        for chain in &f.chains {
            if chain.lifecycle != Lifecycle::Instance
                || !matches!(chain.method.as_str(), "set" | "update" | "try_update")
            {
                continue;
            }
            let mut reasons = Vec::new();
            if chain.inside_loop {
                reasons.push(
                    "written inside a loop (instance entry grows on every iteration)".to_string(),
                );
            }
            if let Some(ty) = resolve_value_type(chain, f, model) {
                if type_looks_collection(&ty) {
                    reasons.push(format!(
                        "value type `{ty}` is an unbounded collection — consider persistent \
                         storage or a bounded encoding"
                    ));
                } else if let Some(fields) = model.struct_field_counts.get(&ty) {
                    if *fields > MAX_INSTANCE_STRUCT_FIELDS {
                        reasons.push(format!(
                            "value type `{ty}` has {fields} fields — instance storage is a single \
                             ledger entry paid on every call"
                        ));
                    }
                }
            }
            if !reasons.is_empty() {
                out.push(RuleFinding {
                    rule: "instance_storage_bloat",
                    severity: Severity::Warning,
                    line: chain.line,
                    column: chain.column,
                    message: format!(
                        "instance storage write may bloat the contract instance ({})",
                        reasons.join("; ")
                    ),
                });
            }
        }
    }
}

/// R4 — `temporary_ttl_exceeded` (warning)
///
/// Extending a temporary entry beyond the network maximum TTL is silently
/// clamped by the host; the effective lifetime is shorter than the code claims.
pub fn temporary_ttl_exceeded(model: &FileModel, out: &mut Vec<RuleFinding>) {
    for f in &model.fns {
        for chain in &f.chains {
            if chain.lifecycle != Lifecycle::Temporary
                || !matches!(chain.method.as_str(), "extend_ttl")
            {
                continue;
            }
            // `extend_ttl(key, threshold, extend_to)` — the target is arg 2.
            let Some(Expr::Lit(lit)) = chain.args.get(2) else {
                continue;
            };
            let syn::Lit::Int(int) = &lit.lit else {
                continue;
            };
            let Ok(target) = int.base10_parse::<u32>() else {
                continue;
            };
            if target > MAX_TEMP_TTL {
                out.push(RuleFinding {
                    rule: "temporary_ttl_exceeded",
                    severity: Severity::Warning,
                    line: chain.line,
                    column: chain.column,
                    message: format!(
                        "extending a temporary entry to {target} ledgers exceeds the network \
                         maximum ({MAX_TEMP_TTL}); the host clamps silently, so the effective \
                         lifetime is shorter than requested"
                    ),
                });
            }
        }
    }
}

type RuleFn = fn(&FileModel, &mut Vec<RuleFinding>);

/// Runs every rule over the model, appending findings in a stable order.
pub fn run_all(
    model: &FileModel,
    ignored: &std::collections::HashSet<String>,
    out: &mut Vec<RuleFinding>,
) {
    let rules: Vec<(&'static str, RuleFn)> = vec![
        ("temporary_critical_type", temporary_critical_type),
        ("lifecycle_mismatch", lifecycle_mismatch),
        ("missing_ttl_extension", missing_ttl_extension),
        ("instance_storage_bloat", instance_storage_bloat),
        ("temporary_ttl_exceeded", temporary_ttl_exceeded),
    ];
    for (name, rule) in rules {
        if ignored.contains(name) {
            continue;
        }
        let mut findings = Vec::new();
        rule(model, &mut findings);
        out.extend(findings);
    }
}
