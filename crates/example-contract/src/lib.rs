//! Example Soroban contract built on `soroban-storage`.
//!
//! A tiny vault that demonstrates every framework feature:
//!
//! - `Balance` — critical user funds in **persistent** storage with auto-bump.
//! - `Nonce` — disposable data in **temporary** storage (safe: easily recreated).
//! - `Admin` — shared contract metadata in **instance** storage.
//! - `Grant` — time-bounded state written with an explicit TTL (`set_ttl`).

#![no_std]
// Tests run on the host where `std` is available.
#[cfg(test)]
extern crate std;

use soroban_sdk::{contract, contracterror, contractimpl, panic_with_error, Address, Env};
use soroban_storage::{storage, TtlPolicy};

storage! {
    /// User vault balance. Irreplaceable — temporary storage would lose funds
    /// permanently on expiry, so this entry is declared `critical`.
    #[storage(persistent, critical)]
    #[storage(policy = TtlPolicy::days(7, 30))]
    Balance(Address) -> i128,

    /// Deposit nonce for replay protection. Cheap, disposable, easily
    /// recreated — the canonical *safe* use of temporary storage.
    #[storage(temporary)]
    Nonce(Address) -> u64,

    /// Contract admin. Shared metadata: instance storage.
    #[storage(instance)]
    Admin() -> Address,

    /// One-time grant, valid for 14 days from the moment it is written.
    #[storage(persistent)]
    Grant(Address) -> i128,
}

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VaultError {
    NotAdmin = 0,
    NonPositiveAmount = 1,
    InsufficientBalance = 2,
    NothingToClaim = 3,
    StaleNonce = 4,
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    /// Initializes the contract and records the admin in instance storage.
    pub fn __constructor(env: Env, admin: Address) {
        admin.require_auth();
        AdminKey::set(&env, &admin);
    }

    /// Adds `amount` to `user`'s balance. The entry TTL is auto-extended to
    /// the policy target (30 days) on every access.
    pub fn deposit(env: Env, user: Address, amount: i128) {
        if amount <= 0 {
            panic_with_error!(&env, VaultError::NonPositiveAmount);
        }
        BalanceKey::update(&env, &user, |bal| bal.unwrap_or(0) + amount);
    }

    /// Withdraws `amount` from `user`'s balance.
    pub fn withdraw(env: Env, user: Address, amount: i128) {
        user.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, VaultError::NonPositiveAmount);
        }
        let balance = BalanceKey::get(&env, &user).unwrap_or(0);
        if balance < amount {
            panic_with_error!(&env, VaultError::InsufficientBalance);
        }
        BalanceKey::set(&env, &user, &(balance - amount));
    }

    /// Returns `user`'s balance.
    pub fn balance(env: Env, user: Address) -> i128 {
        BalanceKey::get(&env, &user).unwrap_or(0)
    }

    /// Returns the admin address.
    pub fn admin(env: Env) -> Address {
        AdminKey::get(&env).expect("uninitialized")
    }

    /// Grants `amount` to `user`, expiring 14 days from now. Only the admin
    /// may grant; the grant lives in persistent storage with an explicit TTL.
    pub fn grant(env: Env, user: Address, amount: i128) {
        AdminKey::get(&env).expect("uninitialized").require_auth();
        if amount <= 0 {
            panic_with_error!(&env, VaultError::NonPositiveAmount);
        }
        let ttl = 14 * soroban_storage::ttl::LEDGERS_PER_DAY;
        GrantKey::set_ttl(&env, &user, &amount, ttl);
    }

    /// Returns whether `user` has a pending grant.
    pub fn has_grant(env: Env, user: Address) -> bool {
        GrantKey::has(&env, &user)
    }

    /// Claims a pending grant before it expires, moving it into the vault
    /// balance.
    pub fn claim(env: Env, user: Address) -> i128 {
        user.require_auth();
        let amount = GrantKey::get(&env, &user).unwrap_or(0);
        if amount == 0 {
            panic_with_error!(&env, VaultError::NothingToClaim);
        }
        GrantKey::remove(&env, &user);
        BalanceKey::update(&env, &user, |bal| bal.unwrap_or(0) + amount);
        amount
    }

    /// Records `nonce`, rejecting any value at or below the previously seen
    /// one. Nonces are disposable and safely stored in temporary storage.
    pub fn consume_nonce(env: Env, user: Address, nonce: u64) {
        user.require_auth();
        let last = NonceKey::get(&env, &user).unwrap_or(0);
        if nonce <= last {
            panic_with_error!(&env, VaultError::StaleNonce);
        }
        NonceKey::set(&env, &user, &nonce);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::storage::Persistent as _;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{Address, Env};

    fn setup(env: &Env) -> (Address, Address, Address, VaultClient<'_>) {
        env.mock_all_auths();
        let admin = Address::generate(env);
        let user = Address::generate(env);
        // `register` runs the constructor (with auth mocked) during registration.
        let contract_id = env.register(Vault, (admin.clone(),));
        let client = VaultClient::new(env, &contract_id);
        (admin, user, contract_id, client)
    }

    #[test]
    fn deposit_withdraw_balance() {
        let env = Env::default();
        let (_admin, user, contract_id, client) = setup(&env);

        assert_eq!(client.balance(&user), 0);

        client.deposit(&user, &1000);
        client.deposit(&user, &500);
        assert_eq!(client.balance(&user), 1500);

        client.withdraw(&user, &400);
        assert_eq!(client.balance(&user), 1100);

        // Deposits auto-extend the balance TTL to the 30-day policy target.
        // Direct storage reads need the contract context.
        let ttl = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get_ttl(&StorageKey::Balance(user.clone()))
        });
        assert!(ttl >= 500_000, "expected ~30-day TTL, got {ttl}");
    }

    #[test]
    fn grant_expires_and_claim_moves_funds() {
        let env = Env::default();
        let (_admin, user, contract_id, client) = setup(&env);

        client.grant(&user, &250);

        // Grant is pinned to 14 days — not the 30-day policy default.
        let grant_ttl = env.as_contract(&contract_id, || {
            env.storage()
                .persistent()
                .get_ttl(&StorageKey::Grant(user.clone()))
        });
        let fourteen_days = 14 * soroban_storage::ttl::LEDGERS_PER_DAY;
        assert!(
            grant_ttl >= fourteen_days - 10 && grant_ttl <= fourteen_days + 10,
            "expected ~14-day grant TTL, got {grant_ttl}"
        );

        let claimed = client.claim(&user);
        assert_eq!(claimed, 250);
        assert_eq!(client.balance(&user), 250);
        assert!(!client.has_grant(&user));
    }

    #[test]
    fn nonce_rejects_stale_values() {
        let env = Env::default();
        let (_admin, user, _contract_id, client) = setup(&env);

        client.consume_nonce(&user, &1);
        client.consume_nonce(&user, &2);
        client.consume_nonce(&user, &5);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client.consume_nonce(&user, &4)
        }));
        assert!(result.is_err(), "stale nonce should panic");
    }

    #[test]
    fn withdraw_beyond_balance_panics() {
        let env = Env::default();
        let (_admin, user, _contract_id, client) = setup(&env);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| client.withdraw(&user, &1)));
        assert!(result.is_err(), "withdraw beyond balance should panic");
    }
}
