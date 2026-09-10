//! Low-level storage operations shared by the [`crate::storage!`] generated
//! accessors.
//!
//! These functions are also usable directly when hand-rolling storage code, but
//! prefer the macro: it binds each key to a single lifecycle at compile time.

use core::fmt::Debug;

use soroban_sdk::{Env, IntoVal, TryFromVal, Val};

use crate::lifecycle::Lifecycle;
use crate::ttl::TtlPolicy;

/// Returns whether an entry exists under `key` in `lifecycle` storage.
pub fn has<K: IntoVal<Env, Val>>(env: &Env, lifecycle: Lifecycle, key: &K) -> bool {
    match lifecycle {
        Lifecycle::Persistent => env.storage().persistent().has(key),
        Lifecycle::Temporary => env.storage().temporary().has(key),
        Lifecycle::Instance => env.storage().instance().has(key),
    }
}

/// Reads the entry under `key`, optionally auto-extending its TTL when present.
///
/// A missing entry is never bumped (there is nothing to extend) and returns
/// `None`.
pub fn get<K, V>(
    env: &Env,
    lifecycle: Lifecycle,
    key: &K,
    policy: &TtlPolicy,
    auto_bump: bool,
) -> Option<V>
where
    K: IntoVal<Env, Val>,
    V: TryFromVal<Env, Val>,
    V::Error: Debug,
{
    let value = match lifecycle {
        Lifecycle::Persistent => env.storage().persistent().get(key),
        Lifecycle::Temporary => env.storage().temporary().get(key),
        Lifecycle::Instance => env.storage().instance().get(key),
    };
    if value.is_some() && auto_bump {
        bump(env, lifecycle, key, policy);
    }
    value
}

/// Writes the entry under `key`, optionally auto-extending its TTL.
pub fn set<K, V>(
    env: &Env,
    lifecycle: Lifecycle,
    key: &K,
    value: &V,
    policy: &TtlPolicy,
    auto_bump: bool,
) where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val>,
{
    match lifecycle {
        Lifecycle::Persistent => env.storage().persistent().set(key, value),
        Lifecycle::Temporary => env.storage().temporary().set(key, value),
        Lifecycle::Instance => env.storage().instance().set(key, value),
    }
    if auto_bump {
        bump(env, lifecycle, key, policy);
    }
}

/// Writes the entry and pins its TTL to exactly `ttl_ledgers`.
///
/// For data with a known, bounded lifetime (claims, offers, grants). An
/// explicit TTL overrides the policy-based auto-bump: callers do **not** get an
/// implicit policy bump on later reads unless they also call [`bump`].
pub fn set_ttl<K, V>(env: &Env, lifecycle: Lifecycle, key: &K, value: &V, ttl_ledgers: u32)
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val>,
{
    match lifecycle {
        Lifecycle::Persistent => env.storage().persistent().set(key, value),
        Lifecycle::Temporary => env.storage().temporary().set(key, value),
        Lifecycle::Instance => env.storage().instance().set(key, value),
    }
    extend_ttl(env, lifecycle, key, ttl_ledgers);
}

/// Loads, transforms and stores the entry under `key`, optionally
/// auto-extending its TTL. Returns the value that was stored.
pub fn update<K, V>(
    env: &Env,
    lifecycle: Lifecycle,
    key: &K,
    f: impl FnOnce(Option<V>) -> V,
    policy: &TtlPolicy,
    auto_bump: bool,
) -> V
where
    K: IntoVal<Env, Val>,
    V: IntoVal<Env, Val> + TryFromVal<Env, Val>,
{
    let value = match lifecycle {
        Lifecycle::Persistent => env.storage().persistent().update(key, f),
        Lifecycle::Temporary => env.storage().temporary().update(key, f),
        Lifecycle::Instance => env.storage().instance().update(key, f),
    };
    if auto_bump {
        bump(env, lifecycle, key, policy);
    }
    value
}

/// Removes the entry under `key`.
pub fn remove<K: IntoVal<Env, Val>>(env: &Env, lifecycle: Lifecycle, key: &K) {
    match lifecycle {
        Lifecycle::Persistent => env.storage().persistent().remove(key),
        Lifecycle::Temporary => env.storage().temporary().remove(key),
        Lifecycle::Instance => env.storage().instance().remove(key),
    }
}

/// Explicitly extends the entry TTL to **at least** `ttl_ledgers` ledgers from now.
///
/// Maps to the SDK's `extend_ttl(key, ttl_ledgers, ttl_ledgers)`: the host only
/// extends when the current TTL is *below* the threshold, so using the target as
/// the threshold yields `max(current, ttl_ledgers)`. For `Instance`, this also
/// extends the contract code and instance entries.
pub fn extend_ttl<K: IntoVal<Env, Val>>(
    env: &Env,
    lifecycle: Lifecycle,
    key: &K,
    ttl_ledgers: u32,
) {
    match lifecycle {
        Lifecycle::Persistent => {
            env.storage()
                .persistent()
                .extend_ttl(key, ttl_ledgers, ttl_ledgers)
        }
        Lifecycle::Temporary => env
            .storage()
            .temporary()
            .extend_ttl(key, ttl_ledgers, ttl_ledgers),
        Lifecycle::Instance => env
            .storage()
            .instance()
            .extend_ttl(ttl_ledgers, ttl_ledgers),
    }
}

/// Applies `policy` to the entry: extends its TTL to `policy.extend_to_ledgers`
/// only when the current TTL is below `policy.threshold_ledgers`.
///
/// The policy is clamped to the lifecycle maximum before use.
pub fn bump<K: IntoVal<Env, Val>>(env: &Env, lifecycle: Lifecycle, key: &K, policy: &TtlPolicy) {
    let policy = policy.clamped_for(lifecycle);
    match lifecycle {
        Lifecycle::Persistent => {
            env.storage().persistent().extend_ttl(
                key,
                policy.threshold_ledgers,
                policy.extend_to_ledgers,
            );
        }
        Lifecycle::Temporary => {
            env.storage().temporary().extend_ttl(
                key,
                policy.threshold_ledgers,
                policy.extend_to_ledgers,
            );
        }
        Lifecycle::Instance => {
            env.storage()
                .instance()
                .extend_ttl(policy.threshold_ledgers, policy.extend_to_ledgers);
        }
    }
}
