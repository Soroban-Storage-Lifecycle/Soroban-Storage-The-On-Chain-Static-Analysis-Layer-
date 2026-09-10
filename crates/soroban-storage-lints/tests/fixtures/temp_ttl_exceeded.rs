//! Violations: temporary TTL extension beyond the network maximum.
use soroban_sdk::Env;

pub fn extend_temp_too_long(env: Env, key: soroban_sdk::Symbol) {
    // 3_110_400 is the network max for temporary entries; this is clamped.
    env.storage().temporary().extend_ttl(&key, 0, 10_000_000);
}

pub fn extend_temp_within_limit(env: Env, key: soroban_sdk::Symbol) {
    // Within the limit: no lint.
    env.storage().temporary().extend_ttl(&key, 0, 1_000_000);
}