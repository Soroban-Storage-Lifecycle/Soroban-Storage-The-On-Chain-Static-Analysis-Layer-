use soroban_sdk::Address;
use soroban_storage::storage;

storage! {
    Balance(Address) -> i128,
}

fn main() {}