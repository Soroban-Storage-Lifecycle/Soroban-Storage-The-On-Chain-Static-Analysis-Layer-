//! Small XDR conversions shared by the tooling.

use stellar_xdr as xdr;

/// A bounded `VecM` from a `Vec`, with the length bound enforced by the XDR
/// type and a readable error instead of a panic.
pub fn vecm<T, const N: u32>(items: Vec<T>) -> Result<xdr::VecM<T, N>, String> {
    items
        .try_into()
        .map_err(|e| format!("too many elements for XDR vector: {e}"))
}

/// The transaction-level source account as a `MuxedAccount`.
pub fn muxed(account: &xdr::AccountId) -> xdr::MuxedAccount {
    match account {
        xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(h)) => {
            xdr::MuxedAccount::Ed25519(h.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vecm_accepts_within_bounds() {
        let v: xdr::VecM<u32, 3> = vecm(vec![1, 2, 3]).unwrap();
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn vecm_rejects_over_bounds() {
        assert!(vecm::<u32, 2>(vec![1, 2, 3]).is_err());
    }

    #[test]
    fn muxed_mirrors_the_ed25519_key() {
        let account = xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(
            [4u8; 32],
        )));
        assert_eq!(
            muxed(&account),
            xdr::MuxedAccount::Ed25519(xdr::Uint256([4u8; 32]))
        );
    }
}
