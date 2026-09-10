//! Violations: critical data written through `.temporary()`.
use soroban_sdk::{Address, Env};

#[derive(soroban_storage::Critical)]
pub struct Balance {
    pub amount: i128,
}

pub fn write_balance_to_temp(env: Env, key: soroban_sdk::Symbol, balance: Balance) {
    // Critical-derived value type in temporary storage -> error.
    env.storage().temporary().set(&key, &balance);
}

pub fn write_funds_to_temp(env: Env, key: soroban_sdk::Symbol, funds: i128) {
    // Name heuristic: `funds` binds to primitive i128, unresolved by type name.
    // The variable name is not the type — skipped (conservative).
    env.storage().temporary().set(&key, &funds);
}

pub fn write_named_type(env: Env, key: soroban_sdk::Symbol, amount: TokenBalance) {
    // Type name heuristic: `TokenBalance` looks critical -> error.
    env.storage().temporary().set(&key, &amount);
}

pub struct TokenBalance {
    pub amount: i128,
}