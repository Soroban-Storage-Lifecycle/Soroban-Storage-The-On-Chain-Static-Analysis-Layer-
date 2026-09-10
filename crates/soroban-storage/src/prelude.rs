//! Convenience re-exports.
//!
//! ```ignore
//! use soroban_storage::prelude::*;
//! ```

pub use crate::critical::Critical;
pub use crate::lifecycle::Lifecycle;
pub use crate::ops;
pub use crate::storage;
pub use crate::ttl::{TtlPolicy, MAX_PERSISTENT_TTL, MAX_TEMP_TTL, MIN_PERSISTENT_TTL};
pub use soroban_storage_macros::Critical; // derive macro — shares the trait's name, macro namespace
