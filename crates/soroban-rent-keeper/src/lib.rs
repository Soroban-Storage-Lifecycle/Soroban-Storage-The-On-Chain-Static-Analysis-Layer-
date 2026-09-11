//! # soroban-rent-keeper
//!
//! A rent-keeper daemon for deployed Soroban contracts: watches entry TTLs via
//! Stellar RPC, computes eviction risk windows, and submits batched
//! `ExtendFootprintTTLOp` transactions **before** entries are evicted.
//!
//! Every contract data entry has a TTL and needs rent paid or it gets evicted:
//! temporary entries are permanently deleted, persistent entries are archived
//! (restorable, but restored to the network minimum of `current ledger +
//! 4095`). In-contract bump-on-access keeps *actively used* data alive; idle
//! data needs an off-chain keeper like this one.
//!
//! ## Architecture
//!
//! - [`risk`] — pure TTL/risk-window and batching logic (fully unit-tested).
//! - [`rpc`] — the [`Rpc`] trait the keeper drives; swap the network
//!   implementation for fakes in tests.
//! - [`keeper`] — the poll/extend loop.
//! - [`metrics`] — Prometheus metrics served over HTTP.
//! - [`stellar`] — the production [`Rpc`] implementation on top of
//!   `stellar-rpc-client` + `stellar-xdr`.
//!
//! ## Configuration
//!
//! See [`config`] for the JSON configuration format; the binary reads
//! `--config config.json`.

pub mod config;
pub mod keeper;
pub mod metrics;
pub mod risk;
pub mod rpc;
pub mod stellar;

pub use keeper::Keeper;
pub use rpc::Rpc;
