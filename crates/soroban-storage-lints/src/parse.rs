//! Extraction of a storage-usage model from parsed Rust source.
//!
//! The linter is intentionally a *syntactic* analyzer: it reads the contract's
//! source (like Slither for Solidity) rather than type-checking it, so it runs
//! instantly in CI without building the contract. Rules are written to be
//! conservative — when a fact cannot be determined, the rule stays quiet and
//! the developer is pointed at the framework guarantees instead.

use std::collections::{HashMap, HashSet};

use proc_macro2::TokenStream as TokenStream2;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Expr, ExprMethodCall, File, ImplItem, Item, Stmt};

/// The three Soroban storage lifecycles (as written in source).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Temporary,
    Persistent,
    Instance,
}

impl Lifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lifecycle::Temporary => "temporary",
            Lifecycle::Persistent => "persistent",
            Lifecycle::Instance => "instance",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "temporary" => Some(Lifecycle::Temporary),
            "persistent" => Some(Lifecycle::Persistent),
            "instance" => Some(Lifecycle::Instance),
            _ => None,
        }
    }
}

/// Storage-chain terminal methods on `env.storage().<lifecycle>()`.
pub const TERMINAL_METHODS: [&str; 9] = [
    "set",
    "get",
    "update",
    "try_update",
    "extend_ttl",
    "extend_ttl_with_limits",
    "bump",
    "remove",
    "has",
];

/// Methods (any receiver) that count as explicit TTL management for a function.
pub const TTL_METHODS: [&str; 4] = ["extend_ttl", "extend_ttl_with_limits", "bump", "set_ttl"];

/// An entry declared inside a `storage!` macro invocation.
#[derive(Debug)]
pub struct StorageDecl {
    pub lifecycle: Lifecycle,
    pub variant: String,
    pub critical: bool,
    pub value_type: String,
    pub line: usize,
    pub column: usize,
}

/// A raw `env.storage().<lifecycle>().<method>(...)` call chain found in source.
pub struct StorageChain {
    pub lifecycle: Lifecycle,
    pub method: String,
    pub args: Vec<Expr>,
    pub line: usize,
    pub column: usize,
    pub inside_loop: bool,
}

/// Storage activity of a single function body.
pub struct FnStorage {
    pub chains: Vec<StorageChain>,
    /// Whether the function contains any explicit TTL-management call
    /// (`extend_ttl`, `bump`, `set_ttl`, ...).
    pub has_ttl_call: bool,
    /// Parameter bindings (`name -> type`) scoped to this function, so value
    /// resolution never leaks across functions with the same parameter names.
    pub bindings: HashMap<String, String>,
    /// The `Self` type when the function lives in an `impl` block.
    pub self_type: Option<String>,
}

/// The full storage model of one source file.
#[derive(Default)]
pub struct FileModel {
    /// Entries declared via `storage!`.
    pub decls: Vec<StorageDecl>,
    /// Storage chains grouped by the function they appear in.
    pub fns: Vec<FnStorage>,
    /// Type names that implement `Critical` in this crate.
    pub critical_types: HashSet<String>,
    /// Struct name -> number of fields.
    pub struct_field_counts: HashMap<String, usize>,
    /// Struct name -> (field name -> field type), for resolving `self.field`
    /// and `value.field` expressions.
    pub struct_field_types: HashMap<String, HashMap<String, String>>,
    /// Best-effort `let name: Type` / parameter bindings, normalized.
    pub bindings: HashMap<String, String>,
}

/// Whether any derive attribute names a `Critical` derive.
fn derives_critical(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        let Ok(list) = attr.parse_args_with(|input: syn::parse::ParseStream| {
            let mut paths = Vec::new();
            while !input.is_empty() {
                let path: syn::Path = input.parse()?;
                paths.push(path);
                if input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                } else {
                    break;
                }
            }
            Ok(paths)
        }) else {
            return false;
        };
        list.iter()
            .any(|p| p.segments.last().is_some_and(|seg| seg.ident == "Critical"))
    })
}

/// Strips whitespace for stable matching of type spellings.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

pub fn type_string(ty: &syn::Type) -> String {
    normalize(&ty.to_token_stream().to_string())
}

/// Builds the storage model for a parsed file.
pub fn scan_file(file: &File) -> FileModel {
    let mut model = FileModel::default();

    // Pass 1: file-level facts (declarations, types, bindings).
    for item in &file.items {
        match item {
            Item::Struct(s) => {
                model
                    .struct_field_counts
                    .insert(s.ident.to_string(), s.fields.len());
                let mut fields = HashMap::new();
                if let syn::Fields::Named(named) = &s.fields {
                    for f in &named.named {
                        if let Some(ident) = &f.ident {
                            fields.insert(ident.to_string(), type_string(&f.ty));
                        }
                    }
                }
                model.struct_field_types.insert(s.ident.to_string(), fields);
                if derives_critical(&s.attrs) {
                    model.critical_types.insert(s.ident.to_string());
                }
            }
            Item::Enum(e) => {
                if derives_critical(&e.attrs) {
                    model.critical_types.insert(e.ident.to_string());
                }
            }
            Item::Impl(imp) => {
                // `impl Critical for X { ... }`
                if let Some((_, path, _)) = &imp.trait_ {
                    if path
                        .segments
                        .last()
                        .is_some_and(|seg| seg.ident == "Critical")
                    {
                        if let syn::Type::Path(tp) = &*imp.self_ty {
                            if let Some(seg) = tp.path.segments.last() {
                                model.critical_types.insert(seg.ident.to_string());
                            }
                        }
                    }
                }
                for it in &imp.items {
                    if let ImplItem::Fn(f) = it {
                        collect_bindings(&f.sig, &f.block, &mut model.bindings);
                    }
                }
            }
            Item::Macro(mac) => {
                if mac
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|seg| seg.ident == "storage")
                {
                    model.decls.extend(parse_storage_decl(&mac.mac.tokens));
                }
            }
            Item::Fn(f) => collect_bindings(&f.sig, &f.block, &mut model.bindings),
            // Trait methods have no body; nothing to collect.
            _ => {}
        }
    }

    // Pass 2: per-function chain scanning.
    for item in &file.items {
        match item {
            Item::Fn(f) => {
                let mut bindings = HashMap::new();
                collect_bindings(&f.sig, &f.block, &mut bindings);
                model.fns.push(scan_fn(&f.block, bindings, None));
            }
            Item::Impl(imp) => {
                let self_type = type_path_last(&imp.self_ty);
                for it in &imp.items {
                    if let ImplItem::Fn(f) = it {
                        let mut bindings = HashMap::new();
                        collect_bindings(&f.sig, &f.block, &mut bindings);
                        model
                            .fns
                            .push(scan_fn(&f.block, bindings, self_type.clone()));
                    }
                }
            }
            _ => {}
        }
    }

    model
}

/// Collects parameter bindings (`name: Type`) from a function signature and
/// recurses into nested functions. `let x: T` type ascription is not exposed by
/// current syn, so this is parameter-driven only — best effort.
fn collect_bindings(sig: &syn::Signature, body: &syn::Block, out: &mut HashMap<String, String>) {
    for input in &sig.inputs {
        if let syn::FnArg::Typed(pat) = input {
            if let syn::Pat::Ident(id) = &*pat.pat {
                out.insert(id.ident.to_string(), type_string(&pat.ty));
            }
        }
    }
    for stmt in &body.stmts {
        if let Stmt::Item(Item::Fn(f)) = stmt {
            collect_bindings(&f.sig, &f.block, out);
        }
    }
}

/// Last path segment of a type, e.g. `soroban_sdk::Address` -> `Address`.
pub fn type_path_last(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
        syn::Type::Reference(r) => type_path_last(&r.elem),
        _ => None,
    }
}

fn scan_fn(
    block: &syn::Block,
    bindings: HashMap<String, String>,
    self_type: Option<String>,
) -> FnStorage {
    let mut visitor = FnVisitor {
        chains: Vec::new(),
        has_ttl_call: false,
        loop_depth: 0,
    };
    visitor.visit_block(block);
    FnStorage {
        chains: visitor.chains,
        has_ttl_call: visitor.has_ttl_call,
        bindings,
        self_type,
    }
}

struct FnVisitor {
    chains: Vec<StorageChain>,
    has_ttl_call: bool,
    loop_depth: usize,
}

impl FnVisitor {
    fn push_chain(&mut self, method: &str, node: &ExprMethodCall) {
        let names = collect_method_names(&node.receiver);
        let Some(lifecycle) = names.iter().find_map(|n| Lifecycle::from_str(n)) else {
            return;
        };
        // Require the chain to go through `env.storage()` to avoid matching
        // unrelated `.temporary()`-named methods.
        if !names.iter().any(|n| n == "storage") {
            return;
        }
        let start = Spanned::span(node).start();
        self.chains.push(StorageChain {
            lifecycle,
            method: method.to_string(),
            args: node.args.iter().cloned().collect(),
            line: start.line,
            column: start.column,
            inside_loop: self.loop_depth > 0,
        });
    }
}

impl<'ast> Visit<'ast> for FnVisitor {
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        if TERMINAL_METHODS.contains(&method.as_str()) {
            self.push_chain(&method, node);
        }
        if TTL_METHODS.contains(&method.as_str()) {
            self.has_ttl_call = true;
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        // `soroban_storage::ops::extend_ttl(...)`-style path calls also count as
        // explicit TTL management.
        if let Expr::Path(p) = &*node.func {
            if let Some(last) = p.path.segments.last() {
                if TTL_METHODS.contains(&last.ident.to_string().as_str()) {
                    self.has_ttl_call = true;
                }
            }
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.loop_depth += 1;
        syn::visit::visit_expr_for_loop(self, node);
        self.loop_depth -= 1;
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.loop_depth += 1;
        syn::visit::visit_expr_while(self, node);
        self.loop_depth -= 1;
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.loop_depth += 1;
        syn::visit::visit_expr_loop(self, node);
        self.loop_depth -= 1;
    }
}

/// Collects the method names along a receiver chain, e.g.
/// `env.storage().persistent()` -> `["persistent", "storage"]`.
fn collect_method_names(expr: &Expr) -> Vec<String> {
    let mut names = Vec::new();
    collect_method_names_into(expr, &mut names);
    names
}

fn collect_method_names_into(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::MethodCall(mc) => {
            names.push(mc.method.to_string());
            collect_method_names_into(&mc.receiver, names);
        }
        Expr::Call(call) => {
            collect_method_names_into(&call.func, names);
        }
        _ => {}
    }
}

/// Re-parses the token stream inside a `storage! { ... }` invocation with a
/// miniature version of the macro grammar — just enough to recover lifecycle,
/// criticality and value types for the rules.
fn parse_storage_decl(tokens: &TokenStream2) -> Vec<StorageDecl> {
    struct Input {
        entries: Vec<Decl>,
    }
    struct Decl {
        attrs: Vec<syn::Attribute>,
        name: syn::Ident,
        value_type: syn::Type,
    }
    impl syn::parse::Parse for Input {
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            let mut entries = Vec::new();
            while !input.is_empty() {
                let attrs = input.call(syn::Attribute::parse_outer)?;
                let name: syn::Ident = input.parse()?;
                let content;
                syn::parenthesized!(content in input);
                while !content.is_empty() {
                    let _: syn::Type = content.parse()?;
                    if content.peek(syn::Token![,]) {
                        content.parse::<syn::Token![,]>()?;
                    } else {
                        break;
                    }
                }
                input.parse::<syn::Token![->]>()?;
                let value_type: syn::Type = input.parse()?;
                input.parse::<syn::Token![,]>()?;
                entries.push(Decl {
                    attrs,
                    name,
                    value_type,
                });
            }
            Ok(Input { entries })
        }
    }

    syn::parse2::<Input>(tokens.clone())
        .map(|input| {
            input
                .entries
                .into_iter()
                .map(|decl| {
                    let mut lifecycle = None;
                    let mut critical = false;
                    for attr in &decl.attrs {
                        if !attr.path().is_ident("storage") {
                            continue;
                        }
                        let Ok(meta) = attr.parse_args_with(|input: syn::parse::ParseStream| {
                            let mut lc = None;
                            let mut cr = false;
                            loop {
                                if input.peek(syn::Ident) {
                                    let ident: syn::Ident = input.parse()?;
                                    match ident.to_string().as_str() {
                                        "persistent" => lc = Some(Lifecycle::Persistent),
                                        "temporary" => lc = Some(Lifecycle::Temporary),
                                        "instance" => lc = Some(Lifecycle::Instance),
                                        "critical" => cr = true,
                                        _ => {}
                                    }
                                }
                                if input.peek(syn::Token![,]) {
                                    input.parse::<syn::Token![,]>()?;
                                } else {
                                    break;
                                }
                            }
                            Ok((lc, cr))
                        }) else {
                            continue;
                        };
                        if lifecycle.is_none() {
                            lifecycle = meta.0;
                        }
                        critical |= meta.1;
                    }
                    let start = decl.name.span().start();
                    StorageDecl {
                        lifecycle: lifecycle.unwrap_or(Lifecycle::Persistent),
                        variant: decl.name.to_string(),
                        critical,
                        value_type: type_string(&decl.value_type),
                        line: start.line,
                        column: start.column,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
