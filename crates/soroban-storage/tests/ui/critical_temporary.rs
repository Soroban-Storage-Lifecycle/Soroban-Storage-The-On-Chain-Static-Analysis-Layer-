use soroban_sdk::Address;
use soroban_storage::storage;

storage! {
    /// User token balance. Critical data can never be temporary.
    #[storage(temporary, critical)]
    Balance(Address) -> i128,
}

fn main() {}