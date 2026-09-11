//! # soroban-restore-planner
//!
//! A CLI that plans the restoration of archived Soroban contract state: given a
//! deployed contract, it probes Stellar RPC for the ledger entries that have
//! been archived, and emits a ready-to-sign `RestoreFootprintOp` transaction
//! that brings them back.
//!
//! ## Why a restore planner?
//!
//! Persistent Soroban entries are *archived* (not deleted) when their TTL
//! expires — unlike temporary entries, which are gone forever. Archived entries
//! can be recovered with a `RestoreFootprintOp`, but the operator must know
//! exactly which ledger keys are archived and provide correct resource data, or
//! the transaction fails. The planner does that discovery and packaging.
//!
//! ## How discovery works
//!
//! `getLedgerEntries` only reports *live* state, so an archived entry is simply
//! absent. The planner therefore builds a **probe** `RestoreFootprintOp`
//! transaction whose read-write footprint contains the candidate entries
//! (the contract instance, its code, and any explicit data keys), simulates it,
//! and reads the authoritative footprint back from the simulation's
//! `restorePreamble`. The final transaction is rebuilt from that resource data.
//!
//! ## Modules
//!
//! - [`keys`] — strkey and `LedgerKey` helpers (pure).
//! - [`plan`] — transaction construction and footprint extraction (pure).
//! - [`rpc`] — the [`Rpc`] trait the planner drives; fake it in tests.
//! - [`planner`] — the end-to-end planning flow over [`Rpc`].
//! - [`stellar`] — the production [`Rpc`] implementation and signing.

pub mod keys;
pub mod plan;
pub mod planner;
pub mod rpc;
pub mod stellar;

pub use plan::{Candidate, PlannedRestore, Simulation};
pub use planner::Planner;
pub use rpc::Rpc;
