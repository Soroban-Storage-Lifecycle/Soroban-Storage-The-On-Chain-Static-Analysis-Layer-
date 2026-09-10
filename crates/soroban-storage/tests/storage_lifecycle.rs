//! Host-based tests verifying lifecycle enforcement and TTL auto-bump behavior
//! of the `storage!` macro against the Soroban host.
//!
//! Direct `env.storage()` access is only permitted inside a contract context,
//! so every test registers a dummy contract and runs inside `env.as_contract`.

use soroban_sdk::contract;
use soroban_sdk::testutils::storage::{Persistent as _, Temporary as _};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};
use soroban_storage::{storage, Critical, Lifecycle, TtlPolicy};

storage! {
    /// User token balance. Irreplaceable; must never be temporary.
    #[storage(persistent, critical)]
    #[storage(policy = TtlPolicy::days(7, 30))]
    Balance(Address) -> i128,

    /// Disposable nonce. Safe in temporary storage.
    #[storage(temporary)]
    Nonce(Address) -> u64,

    /// Contract admin, stored in instance storage.
    #[storage(instance)]
    Admin() -> Address,

    /// Persistent entry with automatic bumping disabled.
    #[storage(persistent, no_bump)]
    Marker(Address) -> u64,
}

/// Dummy contract providing the storage context for tests.
#[contract]
pub struct Dummy;

/// Runs `f` inside a fresh contract context with a registered dummy contract.
fn in_contract<T>(f: impl FnOnce(&Env) -> T) -> T {
    let env = Env::default();
    let id = env.register(Dummy, ());
    env.as_contract(&id, || f(&env))
}

fn addr(env: &Env) -> Address {
    Address::generate(env)
}

// The assertions below verify macro-generated `const`s; clippy's
// `assertions_on_constants` is exactly what makes them useful as regression
// tests.
#[allow(clippy::assertions_on_constants)]
#[test]
fn declarations_expose_lifecycle_at_compile_time() {
    assert_eq!(BalanceKey::LIFECYCLE, Lifecycle::Persistent);
    assert!(BalanceKey::CRITICAL);
    assert!(BalanceKey::AUTO_BUMP);

    assert_eq!(NonceKey::LIFECYCLE, Lifecycle::Temporary);
    assert!(!NonceKey::CRITICAL);

    assert_eq!(AdminKey::LIFECYCLE, Lifecycle::Instance);
    assert!(!AdminKey::CRITICAL);

    assert_eq!(MarkerKey::LIFECYCLE, Lifecycle::Persistent);
    assert!(!MarkerKey::AUTO_BUMP);
}

#[test]
fn set_get_update_remove_roundtrip() {
    in_contract(|env| {
        let user = addr(env);

        assert!(!BalanceKey::has(env, &user));
        assert_eq!(BalanceKey::get(env, &user), None);

        BalanceKey::set(env, &user, &100);
        assert!(BalanceKey::has(env, &user));
        assert_eq!(BalanceKey::get(env, &user), Some(100));

        let updated = BalanceKey::update(env, &user, |b| b.unwrap_or(0) + 5);
        assert_eq!(updated, 105);
        assert_eq!(BalanceKey::get(env, &user), Some(105));

        BalanceKey::remove(env, &user);
        assert!(!BalanceKey::has(env, &user));
        assert_eq!(BalanceKey::get(env, &user), None);
    });
}

#[test]
fn temporary_entry_roundtrip() {
    in_contract(|env| {
        let user = addr(env);

        NonceKey::set(env, &user, &42);
        assert_eq!(NonceKey::get(env, &user), Some(42));

        NonceKey::update(env, &user, |n| n.unwrap_or(0) + 1);
        assert_eq!(NonceKey::get(env, &user), Some(43));

        NonceKey::remove(env, &user);
        assert_eq!(NonceKey::get(env, &user), None);
    });
}

#[test]
fn instance_entry_roundtrip() {
    in_contract(|env| {
        let admin = addr(env);

        assert_eq!(AdminKey::get(env), None);
        AdminKey::set(env, &admin);
        assert_eq!(AdminKey::get(env), Some(admin.clone()));
        AdminKey::remove(env);
        assert_eq!(AdminKey::get(env), None);
    });
}

#[test]
fn auto_bump_extends_persistent_ttl_to_policy() {
    in_contract(|env| {
        let user = addr(env);

        BalanceKey::set(env, &user, &10);
        let ttl = env
            .storage()
            .persistent()
            .get_ttl(&StorageKey::Balance(user.clone()));
        // Policy: extend to 30 days (~518_400 ledgers) on access.
        assert!(
            ttl >= 500_000,
            "expected policy-extended TTL (~30 days), got {ttl} ledgers"
        );

        // A read also re-bumps while the entry exists.
        let _ = BalanceKey::get(env, &user);
        let ttl_after_read = env
            .storage()
            .persistent()
            .get_ttl(&StorageKey::Balance(user.clone()));
        assert!(ttl_after_read >= 500_000);
    });
}

#[test]
fn temporary_ttl_respects_max() {
    in_contract(|env| {
        let user = addr(env);

        NonceKey::set(env, &user, &1);
        let ttl = env
            .storage()
            .temporary()
            .get_ttl(&StorageKey::Nonce(user.clone()));
        assert!(
            (500_000..=soroban_storage::ttl::MAX_TEMP_TTL).contains(&ttl),
            "temporary TTL out of expected range: {ttl}"
        );
    });
}

#[test]
fn no_bump_keeps_minimum_ttl() {
    in_contract(|env| {
        let user = addr(env);

        MarkerKey::set(env, &user, &7);
        let ttl = env
            .storage()
            .persistent()
            .get_ttl(&StorageKey::Marker(user.clone()));
        // Without a bump the entry keeps whatever the host assigned on creation
        // (the network minimum), far below the 30-day policy target.
        assert!(ttl < 10_000, "expected un-bumped TTL, got {ttl}");
    });
}

#[test]
fn set_ttl_pins_exact_lifetime() {
    in_contract(|env| {
        let user = addr(env);

        BalanceKey::set_ttl(env, &user, &50, 100_000);
        let ttl = env
            .storage()
            .persistent()
            .get_ttl(&StorageKey::Balance(user.clone()));
        assert!(
            ttl >= 99_990,
            "expected pinned TTL ~100_000 ledgers, got {ttl}"
        );
    });
}

#[test]
fn explicit_extend_ttl_works_without_auto_bump() {
    in_contract(|env| {
        let user = addr(env);

        MarkerKey::set(env, &user, &7);
        MarkerKey::extend_ttl(env, &user, 50_000);
        let ttl = env
            .storage()
            .persistent()
            .get_ttl(&StorageKey::Marker(user.clone()));
        assert!(
            ttl >= 49_990,
            "expected extended TTL ~50_000 ledgers, got {ttl}"
        );
    });
}

#[test]
fn explicit_bump_applies_policy() {
    in_contract(|env| {
        let user = addr(env);

        MarkerKey::set(env, &user, &7);
        MarkerKey::bump(env, &user);
        let ttl = env
            .storage()
            .persistent()
            .get_ttl(&StorageKey::Marker(user.clone()));
        assert!(
            ttl >= 500_000,
            "expected policy TTL after explicit bump, got {ttl}"
        );
    });
}

#[test]
fn critical_marker_trait_derives() {
    #[derive(Critical)]
    struct Balance;

    fn assert_critical<T: Critical>() {}
    assert_critical::<Balance>();
}
