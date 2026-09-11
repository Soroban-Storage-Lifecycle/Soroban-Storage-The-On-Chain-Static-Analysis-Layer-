//! Violations: persistent/instance TTL extension beyond the network maximum.
use soroban_sdk::Env;

pub fn extend_persistent_too_long(env: Env, key: soroban_sdk::Symbol) {
    // 6_311_390 is the network max for persistent entries; this is clamped.
    env.storage().persistent().extend_ttl(&key, 0, 7_000_000);
}

pub fn extend_instance_too_long(env: Env, key: soroban_sdk::Symbol) {
    // Instance entries share the persistent maximum.
    env.storage().instance().extend_ttl(&key, 0, 9_000_000);
}

pub fn extend_persistent_within_limit(env: Env, key: soroban_sdk::Symbol) {
    // Within the limit: no lint.
    env.storage().persistent().extend_ttl(&key, 0, 518_400);
}
