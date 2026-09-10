//! Violations: instance storage bloat.
use soroban_sdk::{Env, Symbol, Vec};

pub fn store_vec_in_instance(env: Env, key: Symbol, values: Vec<i128>) {
    // Unbounded collection in instance storage -> warning.
    env.storage().instance().set(&key, &values);
}

pub fn big_struct_in_instance(env: Env, key: Symbol, cfg: BigConfig) {
    // Struct with more than 3 fields in instance storage -> warning.
    env.storage().instance().set(&key, &cfg);
}

pub struct BigConfig {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
}

pub fn small_struct_in_instance(env: Env, key: Symbol, cfg: SmallConfig) {
    // 2 fields: acceptable -> no lint.
    env.storage().instance().set(&key, &cfg);
}

pub struct SmallConfig {
    pub a: u32,
    pub b: u32,
}

pub fn loop_write(env: Env, key: Symbol) {
    // Instance write inside a loop -> warning (entry grows each iteration).
    for i in 0..10 {
        env.storage().instance().set(&key, &i);
    }
}