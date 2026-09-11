//! Violations caught through the type-aware resolver: struct literals, struct
//! fields via `Self`, and collection element types.
use soroban_sdk::{Address, Env, Map, Vec};

#[derive(soroban_storage::Critical)]
pub struct Balance {
    pub amount: i128,
}

pub struct TokenBalance {
    pub amount: i128,
}

pub struct Price {
    pub value: i128,
}

pub struct Vault {
    pub balance: Balance,
}

impl Vault {
    pub fn write_self_field_to_temp(&self, env: Env, key: soroban_sdk::Symbol) {
        // `self.balance` resolves to `Balance` (Critical) via the impl's Self type.
        env.storage().temporary().set(&key, &self.balance);
    }
}

pub fn write_struct_literal(env: Env, key: soroban_sdk::Symbol) {
    // Struct literal type is known from syntax alone.
    env.storage().temporary().set(&key, &TokenBalance { amount: 5 });
}

pub fn write_map_element(env: Env, key: soroban_sdk::Symbol, balances: Map<Address, Balance>, user: Address) {
    // `balances.get(user)` resolves through `Map<Address, Balance>` -> `Balance`.
    env.storage().temporary().set(&key, &balances.get(user));
}

pub fn write_indexed_element(env: Env, key: soroban_sdk::Symbol, values: Vec<Balance>) {
    // `values[0]` resolves through `Vec<Balance>` -> `Balance`.
    env.storage().temporary().set(&key, &values[0]);
}

pub fn write_price_to_temp(env: Env, key: soroban_sdk::Symbol, prices: Map<Address, Price>, user: Address) {
    // `Price` is not critical: must NOT be flagged (negative control).
    env.storage().temporary().set(&key, &prices.get(user));
}