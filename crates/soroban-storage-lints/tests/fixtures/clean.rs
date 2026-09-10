//! Clean fixture: framework accessors (auto-bump) and correct raw usage.
use soroban_sdk::{Address, Env};
use soroban_storage::{storage, TtlPolicy};

storage! {
    #[storage(persistent, critical)]
    #[storage(policy = TtlPolicy::days(7, 30))]
    Balance(Address) -> i128,

    #[storage(temporary)]
    Nonce(Address) -> u64,

    #[storage(instance)]
    Admin() -> Address,
}

pub fn deposit(env: Env, user: Address, amount: i128) {
    // Framework accessor: lifecycle enforced + auto-bump. No lint should fire.
    BalanceKey::update(&env, &user, |bal| bal.unwrap_or(0) + amount);
}

pub fn withdraw(env: Env, user: Address, amount: i128) {
    let balance: i128 = BalanceKey::get(&env, &user).unwrap_or(0);
    BalanceKey::set(&env, &user, &(balance - amount));
}

pub fn admin(env: Env) -> Address {
    AdminKey::get(&env).unwrap_or_else(|| panic!("uninitialized"))
}

/// Hand-rolled persistent write WITH explicit TTL management: no lint.
pub fn raw_with_extend(env: Env, key: soroban_sdk::Symbol, amount: i128) {
    env.storage().persistent().set(&key, &amount);
    env.storage().persistent().extend_ttl(&key, 17_280, 518_400);
}

pub fn nonce(env: Env, user: Address) -> u64 {
    NonceKey::get(&env, &user).unwrap_or(0)
}