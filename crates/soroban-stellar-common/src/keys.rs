//! Strkey decoding and `LedgerKey` construction helpers.
//!
//! Everything here is pure (no network) and unit-tested.

use stellar_xdr as xdr;
use stellar_xdr::{LedgerEntry, LedgerEntryData, Limits, ReadXdr, ScVal, WriteXdr};

/// Decodes a `C...` contract-id strkey into its 32 raw bytes.
pub fn decode_contract_id(strkey: &str) -> Result<[u8; 32], String> {
    match stellar_strkey::Strkey::from_string(strkey) {
        Ok(stellar_strkey::Strkey::Contract(bytes)) => Ok(bytes.0),
        Ok(_) => Err(format!("{strkey} is not a contract id (`C...`) strkey")),
        Err(e) => Err(format!("invalid contract id `{strkey}`: {e}")),
    }
}

/// The `G...` strkey of an ed25519 account id.
pub fn account_strkey(account: &xdr::AccountId) -> String {
    match account {
        xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(h)) => {
            // `ed25519::PublicKey` has an inherent `to_string` returning a
            // heapless string, so go through `Display` for a std `String`.
            format!("{}", stellar_strkey::ed25519::PublicKey(h.0))
        }
    }
}

/// Decodes a `G...` account-id strkey into an XDR `AccountId`.
pub fn decode_account_id(strkey: &str) -> Result<xdr::AccountId, String> {
    match stellar_strkey::Strkey::from_string(strkey) {
        Ok(stellar_strkey::Strkey::PublicKeyEd25519(pk)) => Ok(xdr::AccountId(
            xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(pk.0)),
        )),
        Ok(_) => Err(format!("{strkey} is not an account (`G...`) strkey")),
        Err(e) => Err(format!("invalid account id `{strkey}`: {e}")),
    }
}

/// The ledger key of a contract's instance entry.
pub fn contract_instance_key(contract_id: [u8; 32]) -> xdr::LedgerKey {
    xdr::LedgerKey::ContractData(xdr::LedgerKeyContractData {
        contract: xdr::ScAddress::Contract(xdr::ContractId(xdr::Hash(contract_id))),
        key: ScVal::LedgerKeyContractInstance,
        durability: xdr::ContractDataDurability::Persistent,
    })
}

/// The ledger key of a contract's wasm code entry.
pub fn contract_code_key(wasm_hash: [u8; 32]) -> xdr::LedgerKey {
    xdr::LedgerKey::ContractCode(xdr::LedgerKeyContractCode {
        hash: xdr::Hash(wasm_hash),
    })
}

/// A persistent contract-data ledger key for `key`.
pub fn contract_data_key(contract_id: [u8; 32], key: ScVal) -> xdr::LedgerKey {
    xdr::LedgerKey::ContractData(xdr::LedgerKeyContractData {
        contract: xdr::ScAddress::Contract(xdr::ContractId(xdr::Hash(contract_id))),
        key,
        durability: xdr::ContractDataDurability::Persistent,
    })
}

/// Decodes a base64-XDR `LedgerKey`.
pub fn decode_ledger_key(base64: &str) -> Result<xdr::LedgerKey, String> {
    xdr::LedgerKey::from_xdr_base64(base64, Limits::none())
        .map_err(|e| format!("invalid ledger key XDR: {e}"))
}

/// Encodes a `LedgerKey` as base64-XDR.
pub fn encode_ledger_key(key: &xdr::LedgerKey) -> Result<String, String> {
    key.to_xdr_base64(Limits::none()).map_err(|e| e.to_string())
}

/// Extracts the wasm hash from a contract instance entry, when present.
pub fn wasm_hash_from_entry(entry: &LedgerEntry) -> Option<[u8; 32]> {
    if let LedgerEntryData::ContractData(d) = &entry.data {
        if let ScVal::ContractInstance(inst) = &d.val {
            if let xdr::ContractExecutable::Wasm(h) = &inst.executable {
                return Some(h.0);
            }
        }
    }
    None
}

/// A short human-readable description of a ledger key, for CLI output.
pub fn describe_key(key: &xdr::LedgerKey) -> String {
    match key {
        xdr::LedgerKey::ContractData(d) => match &d.key {
            ScVal::LedgerKeyContractInstance => "contract instance".to_string(),
            other => format!("contract data {:?}", other),
        },
        xdr::LedgerKey::ContractCode(_) => "contract code".to_string(),
        other => other.name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_contract() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn decodes_contract_strkey() {
        let id = sample_contract();
        let strkey = stellar_strkey::Contract(id).to_string();
        assert_eq!(decode_contract_id(&strkey).unwrap(), id);
    }

    #[test]
    fn rejects_non_contract_strkey() {
        // An ed25519 public key is a valid strkey but not a contract id.
        let account = stellar_strkey::ed25519::PublicKey([1u8; 32]).to_string();
        assert!(decode_contract_id(&account).is_err());
        assert!(decode_contract_id("not-a-strkey").is_err());
    }

    #[test]
    fn instance_key_roundtrips_through_base64() {
        let key = contract_instance_key(sample_contract());
        let encoded = encode_ledger_key(&key).unwrap();
        assert_eq!(decode_ledger_key(&encoded).unwrap(), key);
    }

    #[test]
    fn code_key_describes_itself() {
        let key = contract_code_key([9u8; 32]);
        assert_eq!(describe_key(&key), "contract code");
        let encoded = encode_ledger_key(&key).unwrap();
        assert_eq!(decode_ledger_key(&encoded).unwrap(), key);
    }

    #[test]
    fn instance_key_describes_itself() {
        let key = contract_instance_key(sample_contract());
        assert_eq!(describe_key(&key), "contract instance");
    }

    #[test]
    fn account_strkey_is_a_g_address() {
        let account = xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(
            [3u8; 32],
        )));
        let s = account_strkey(&account);
        assert!(s.starts_with('G'));
        assert_eq!(s.len(), 56);
    }

    #[test]
    fn account_id_roundtrips() {
        let account = xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(
            [3u8; 32],
        )));
        assert_eq!(
            decode_account_id(&account_strkey(&account)).unwrap(),
            account
        );
    }

    #[test]
    fn rejects_contract_strkey_as_account() {
        let contract = stellar_strkey::Contract([1u8; 32]).to_string();
        assert!(decode_account_id(&contract).is_err());
    }
}
