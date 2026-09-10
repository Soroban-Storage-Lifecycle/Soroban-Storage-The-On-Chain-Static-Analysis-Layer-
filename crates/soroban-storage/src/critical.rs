//! Marker trait for irreplaceable data.

/// Marks a type as **critical**: user funds, balances, positions, or any state
/// whose loss is irreversible and cannot be recreated.
///
/// Critical data must never live in [`Lifecycle::Temporary`] storage — a
/// temporary entry is permanently deleted when its TTL expires and can never be
/// restored, so an expired `temporary` balance means lost funds.
///
/// # Usage
///
/// ```ignore
/// use soroban_storage::Critical;
///
/// #[derive(Critical)]
/// pub struct Balance { pub amount: i128 }
/// ```
///
/// In [`crate::storage!`] declarations, mark the *entry* instead:
///
/// ```ignore
/// storage! {
///     #[storage(persistent, critical)]
///     Balance(Address) -> i128,
/// }
/// ```
///
/// Entries marked `critical` are rejected at compile time if declared
/// `temporary`. The `soroban-storage-lints` analyzer additionally flags
/// `Critical`-derived types (and name-shaped fund types) written through raw
/// `env.storage().temporary()` calls.
pub trait Critical {}
