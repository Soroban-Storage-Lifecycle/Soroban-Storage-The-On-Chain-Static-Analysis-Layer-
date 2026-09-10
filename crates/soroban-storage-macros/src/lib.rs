//! Procedural macros for `soroban-storage`.
//!
//! Exposes:
//! - [`storage!`]: declares contract storage entries with a mandatory, explicit
//!   lifecycle (`persistent` | `temporary` | `instance`) and generates typed,
//!   auto-bumping accessors. Critical entries are rejected at compile time if
//!   declared `temporary`.
//! - [`Critical`]: marker derive used to tag value types that hold user funds /
//!   irreplaceable state, so tooling can reject them in temporary storage.
#![deny(missing_docs)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Attribute, Expr, Ident, Result, Token, Type};

// ---------------------------------------------------------------------------
// `storage!` macro
// ---------------------------------------------------------------------------

/// Declares the storage schema of a Soroban contract.
///
/// Every entry is declared with exactly one lifecycle. The macro generates:
///
/// - a `#[soroban_sdk::contracttype]` `StorageKey` enum (one variant per entry),
/// - one accessor struct per entry (e.g. `BalanceKey`) with `has`/`get`/`set`/
///   `set_ttl`/`update`/`remove`/`bump`/`extend_ttl` methods.
///
/// `get`, `set` and `update` automatically extend the entry TTL on access
/// according to the entry's [`TtlPolicy`](https://docs.rs/soroban-storage/latest/soroban_storage/struct.TtlPolicy.html).
///
/// # Syntax
///
/// ```ignore
/// soroban_storage::storage! {
///     /// User token balance. Irreplaceable; must never be temporary.
///     #[storage(persistent, critical)]
///     #[storage(policy = soroban_storage::TtlPolicy::days(7, 30))]
///     Balance(Address) -> i128,
///
///     #[storage(temporary)]
///     Nonce(Address) -> u64,
///
///     #[storage(instance)]
///     Admin() -> Address,
/// }
/// ```
///
/// Options accepted inside `#[storage(...)]`:
/// - `persistent` | `temporary` | `instance` — the lifecycle (required).
/// - `critical` — marks irreplaceable data; rejected with `temporary`.
/// - `auto_bump` / `no_bump` — toggle automatic TTL extension on access (on by default).
/// - `policy = <const TtlPolicy expr>` — custom bump policy.
///
/// # Compile-time guarantees
///
/// - A `critical` entry declared `temporary` is a hard compile error.
/// - Entries without a lifecycle declaration are a hard compile error.
#[proc_macro]
pub fn storage(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as StorageInput);
    match gen_storage(&input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

// --- input model ------------------------------------------------------------

struct StorageInput {
    entries: Vec<Entry>,
}

struct Entry {
    attrs: Vec<Attribute>,
    name: Ident,
    arg_types: Vec<Type>,
    value_type: Type,
}

impl Parse for StorageInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut entries = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(Attribute::parse_outer)?;
            let name: Ident = input.parse()?;
            let content;
            syn::parenthesized!(content in input);
            let mut arg_types = Vec::new();
            if !content.is_empty() {
                arg_types.push(content.parse()?);
                while content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                    if content.is_empty() {
                        break;
                    }
                    arg_types.push(content.parse()?);
                }
            }
            input.parse::<Token![->]>()?;
            let value_type: Type = input.parse()?;
            input.parse::<Token![,]>()?;
            entries.push(Entry {
                attrs,
                name,
                arg_types,
                value_type,
            });
        }
        Ok(StorageInput { entries })
    }
}

// --- storage option parsing -------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum LifecycleKw {
    Temporary,
    Persistent,
    Instance,
}

impl LifecycleKw {
    fn path(self) -> TokenStream2 {
        match self {
            LifecycleKw::Temporary => quote!(::soroban_storage::Lifecycle::Temporary),
            LifecycleKw::Persistent => quote!(::soroban_storage::Lifecycle::Persistent),
            LifecycleKw::Instance => quote!(::soroban_storage::Lifecycle::Instance),
        }
    }
}

#[derive(Default)]
struct StorageOpts {
    lifecycle: Option<LifecycleKw>,
    critical: bool,
    auto_bump: bool,
    policy: Option<TokenStream2>,
}

/// Parses the comma-separated tokens inside `#[storage(...)]`.
impl Parse for StorageOpts {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut opts = StorageOpts {
            auto_bump: true,
            ..Default::default()
        };
        loop {
            if input.peek(Ident) {
                let ident: Ident = input.parse()?;
                let kw = ident.to_string();
                match kw.as_str() {
                    "persistent" => opts.lifecycle = Some(LifecycleKw::Persistent),
                    "temporary" => opts.lifecycle = Some(LifecycleKw::Temporary),
                    "instance" => opts.lifecycle = Some(LifecycleKw::Instance),
                    "critical" => opts.critical = true,
                    "auto_bump" => {
                        opts.auto_bump = true;
                        if input.peek(Token![=]) {
                            input.parse::<Token![=]>()?;
                            let b: syn::LitBool = input.parse()?;
                            opts.auto_bump = b.value;
                        }
                    }
                    "no_bump" => opts.auto_bump = false,
                    "policy" => {
                        input.parse::<Token![=]>()?;
                        let expr: Expr = input.parse()?;
                        opts.policy = Some(quote!(#expr));
                    }
                    other => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!(
                                "unknown storage option `{other}` (expected `persistent`, `temporary`, `instance`, `critical`, `auto_bump`, `no_bump`, or `policy = ...`)"
                            ),
                        ));
                    }
                }
            } else {
                break;
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(opts)
    }
}

fn merge_opts(attrs: &[Attribute]) -> Result<StorageOpts> {
    let mut merged = StorageOpts::default();
    for attr in attrs {
        if attr.path().is_ident("storage") {
            let parsed = attr.parse_args::<StorageOpts>()?;
            if let Some(lc) = parsed.lifecycle {
                merged.lifecycle = Some(lc);
            }
            merged.critical |= parsed.critical;
            merged.auto_bump = parsed.auto_bump;
            if parsed.policy.is_some() {
                merged.policy = parsed.policy;
            }
        }
    }
    Ok(merged)
}

// --- code generation ---------------------------------------------------------

fn gen_storage(input: &StorageInput) -> Result<TokenStream2> {
    if input.entries.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`storage!` requires at least one entry",
        ));
    }

    let key_enum = Ident::new("StorageKey", proc_macro2::Span::call_site());
    let mut variants = Vec::new();
    let mut structs = Vec::new();

    for entry in &input.entries {
        let entry_name = &entry.name;
        let entry_arg_types = &entry.arg_types;
        let value_type = &entry.value_type;
        let key_struct = Ident::new(&format!("{}Key", entry.name), entry.name.span());
        let opts = merge_opts(&entry.attrs)?;

        let lifecycle = match opts.lifecycle {
            Some(lc) => lc,
            None => {
                return Err(syn::Error::new_spanned(
                    &entry.name,
                    "storage entry requires an explicit lifecycle: add `#[storage(persistent)]`, `#[storage(temporary)]`, or `#[storage(instance)]`",
                ));
            }
        };

        if opts.critical && lifecycle == LifecycleKw::Temporary {
            return Err(syn::Error::new_spanned(
                &entry.name,
                "critical entry declared `temporary`: temporary entries are permanently deleted when their TTL expires and can never be restored. Use `persistent` (or `instance`) for data that cannot be recreated.",
            ));
        }

        // Enum variant: `Balance(Address)` or `Admin`.
        let variant_tokens = if entry_arg_types.is_empty() {
            quote!(#entry_name)
        } else {
            quote!(#entry_name(#(#entry_arg_types),*))
        };
        variants.push(variant_tokens);

        // Key constructor: `key(&arg0, &arg1) -> StorageKey`.
        let arg_defs = entry_arg_types
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                let arg_name = Ident::new(&format!("arg{i}"), entry.name.span());
                quote!(#arg_name: &#ty)
            })
            .collect::<Vec<_>>();
        let arg_names = (0..entry_arg_types.len())
            .map(|i| Ident::new(&format!("arg{i}"), entry.name.span()))
            .collect::<Vec<_>>();
        let key_ctor = if entry_arg_types.is_empty() {
            quote!(#key_enum::#entry_name)
        } else {
            quote!(#key_enum::#entry_name(#(#arg_names.clone()),*))
        };

        let lifecycle_path = lifecycle.path();
        let policy = opts
            .policy
            .clone()
            .unwrap_or_else(|| quote!(::soroban_storage::ttl::default_policy(#lifecycle_path)));
        let critical_lit = opts.critical;
        let auto_bump_lit = opts.auto_bump;

        let doc_attrs = entry
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("doc"))
            .collect::<Vec<_>>();

        // Parameter lists are assembled as vectors so the separator never leaks
        // a dangling comma when an entry has zero key args (e.g. `Admin()`).
        let env_param = quote!(env: &::soroban_sdk::Env);
        let key_only = {
            let mut p = vec![env_param.clone()];
            p.extend(arg_defs.iter().cloned());
            p
        };
        let mut set_params = key_only.clone();
        set_params.push(quote!(val: &#value_type));
        let mut set_ttl_params = set_params.clone();
        set_ttl_params.push(quote!(ttl_ledgers: u32));
        let mut update_params = key_only.clone();
        update_params.push(quote!(f: impl FnOnce(Option<#value_type>) -> #value_type));
        let mut extend_ttl_params = key_only.clone();
        extend_ttl_params.push(quote!(ttl_ledgers: u32));
        let call_args = quote!(#(#arg_names),*);

        let generated = quote! {
            #(#doc_attrs)*
            #[derive(Clone, Debug, Eq, PartialEq)]
            pub struct #key_struct;

            impl #key_struct {
                /// The storage lifecycle this entry is bound to at compile time.
                pub const LIFECYCLE: ::soroban_storage::Lifecycle = #lifecycle_path;
                /// Whether this entry holds irreplaceable data.
                pub const CRITICAL: bool = #critical_lit;
                /// Whether accessors automatically extend the entry TTL.
                pub const AUTO_BUMP: bool = #auto_bump_lit;
                /// The TTL bump policy applied on access (when `AUTO_BUMP`).
                pub const POLICY: ::soroban_storage::TtlPolicy = #policy;

                /// Builds the ledger `StorageKey` for this entry.
                pub fn key(#(#arg_defs),*) -> #key_enum {
                    #key_ctor
                }

                /// Returns whether an entry exists under this key.
                pub fn has(#(#key_only),*) -> bool {
                    ::soroban_storage::ops::has(env, Self::LIFECYCLE, &Self::key(#call_args))
                }

                /// Reads the entry, auto-extending its TTL when present.
                pub fn get(#(#key_only),*) -> Option<#value_type> {
                    ::soroban_storage::ops::get(env, Self::LIFECYCLE, &Self::key(#call_args), &Self::POLICY, Self::AUTO_BUMP)
                }

                /// Writes the entry, auto-extending its TTL.
                pub fn set(#(#set_params),*) {
                    ::soroban_storage::ops::set(env, Self::LIFECYCLE, &Self::key(#call_args), val, &Self::POLICY, Self::AUTO_BUMP)
                }

                /// Writes the entry and pins its TTL to `ttl_ledgers`.
                ///
                /// Use this for data with a known, bounded lifetime (claims,
                /// offers, time-limited grants). The explicit TTL overrides the
                /// policy-based auto-bump.
                pub fn set_ttl(#(#set_ttl_params),*) {
                    ::soroban_storage::ops::set_ttl(env, Self::LIFECYCLE, &Self::key(#call_args), val, ttl_ledgers)
                }

                /// Loads, transforms and stores the entry, auto-extending its TTL.
                pub fn update(#(#update_params),*) -> #value_type {
                    ::soroban_storage::ops::update(env, Self::LIFECYCLE, &Self::key(#call_args), f, &Self::POLICY, Self::AUTO_BUMP)
                }

                /// Removes the entry.
                pub fn remove(#(#key_only),*) {
                    ::soroban_storage::ops::remove(env, Self::LIFECYCLE, &Self::key(#call_args))
                }

                /// Explicitly applies the entry's TTL bump policy.
                pub fn bump(#(#key_only),*) {
                    ::soroban_storage::ops::bump(env, Self::LIFECYCLE, &Self::key(#call_args), &Self::POLICY)
                }

                /// Explicitly extends the entry TTL to at least `ttl_ledgers`.
                pub fn extend_ttl(#(#extend_ttl_params),*) {
                    ::soroban_storage::ops::extend_ttl(env, Self::LIFECYCLE, &Self::key(#call_args), ttl_ledgers)
                }
            }
        };
        structs.push(generated);
    }

    Ok(quote! {
        /// Ledger storage keys for this contract, one variant per declared entry.
        #[derive(Clone, Debug, Eq, PartialEq)]
        #[::soroban_sdk::contracttype]
        pub enum #key_enum {
            #(#variants),*
        }
        #(#structs)*
    })
}

// ---------------------------------------------------------------------------
// `Critical` derive
// ---------------------------------------------------------------------------

/// Marks a type as irreplaceable data (user funds, balances, positions...).
///
/// ```ignore
/// #[derive(soroban_storage::Critical)]
/// pub struct Balance { pub amount: i128 }
/// ```
///
/// Used by [`storage!`] declarations (`#[storage(critical)]`) and by the
/// `soroban-storage-lints` static analysis to reject such types in temporary
/// storage.
#[proc_macro_derive(Critical)]
pub fn derive_critical(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let name = &input.ident;
    quote!(impl ::soroban_storage::Critical for #name {}).into()
}
