//! # soroban-stellar-common
//!
//! Shared helpers for the off-chain `soroban-storage` tooling. The rent keeper
//! and the restore planner both need to turn `C...`/`G...`/`S...` strkeys into
//! XDR types, build and encode ledger keys, and sign Soroban transactions.
//! Keeping one tested implementation of that logic — especially the signing
//! payload — is safer than letting two copies drift.
//!
//! ## Modules
//!
//! - [`keys`] — strkey decoding and `LedgerKey` construction.
//! - [`sign`] — keypair derivation and transaction signatures.
//! - [`xdr_util`] — small XDR conversions (`VecM`, `MuxedAccount`).

pub mod keys;
pub mod sign;
pub mod xdr_util;

pub use keys::{
    account_strkey, contract_code_key, contract_data_key, contract_instance_key, decode_account_id,
    decode_contract_id, decode_ledger_key, describe_key, encode_ledger_key, wasm_hash_from_entry,
};
pub use sign::{
    account_from_secret, decorated_signature, keypair_from_secret, sign_envelope,
    signature_payload, SigningKey,
};
pub use xdr_util::{muxed, vecm};
