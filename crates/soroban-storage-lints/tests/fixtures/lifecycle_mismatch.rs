//! Violations: storage!-declared keys accessed through the wrong lifecycle.
use soroban_sdk::{Address, Env};
use soroban_storage::storage;

storage! {
    #[storage(persistent, critical)]
    Balance(Address) -> i128,

    #[storage(temporary)]
    Nonce(Address) -> u64,
}

pub fn wrong_lifecycle_read(env: Env, user: Address) -> i128 {
    // `Balance` is declared persistent but read through `.temporary()`.
    env.storage().temporary().get(&StorageKey::Balance(user))
        .unwrap_or(0)
}

pub fn wrong_lifecycle_write(env: Env, user: Address, amount: i128) {
    // `Nonce` is declared temporary but written through `.persistent()`.
    env.storage().persistent().set(&StorageKey::Nonce(user), &amount);
}