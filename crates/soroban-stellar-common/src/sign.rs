//! Keypair derivation and transaction signatures.
//!
//! The signature payload for a V1 transaction is the XDR of
//! `TransactionSignaturePayload` — the `sha256(passphrase)` network id plus the
//! tagged transaction — hashed again with SHA-256. Getting this wrong produces
//! transactions the network silently rejects, so it lives in exactly one place.

use ed25519_dalek::{Signature, Signer, VerifyingKey};
use sha2::{Digest, Sha256};
use stellar_xdr as xdr;
use stellar_xdr::{Limits, WriteXdr};

use crate::xdr_util::vecm;

pub use ed25519_dalek::SigningKey;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Derives the account id and signing key from an `S...` strkey.
pub fn keypair_from_secret(secret: &str) -> Result<(xdr::AccountId, SigningKey), String> {
    let strkey = stellar_strkey::Strkey::from_string(secret)
        .map_err(|e| format!("invalid signer secret: {e}"))?;
    let bytes = match strkey {
        stellar_strkey::Strkey::PrivateKeyEd25519(bytes) => bytes.0,
        _ => return Err("signer secret must be a private key (`S...`) strkey".to_string()),
    };
    let signing_key = SigningKey::from_bytes(&bytes);
    let verifying = signing_key.verifying_key().to_bytes();
    let account = xdr::AccountId(xdr::PublicKey::PublicKeyTypeEd25519(xdr::Uint256(
        verifying,
    )));
    Ok((account, signing_key))
}

/// The `G...` account that a given `S...` signer secret controls.
pub fn account_from_secret(secret: &str) -> Result<xdr::AccountId, String> {
    keypair_from_secret(secret).map(|(account, _)| account)
}

/// The SHA-256 signature payload for a V1 transaction.
pub fn signature_payload(tx: &xdr::Transaction, passphrase: &str) -> Result<[u8; 32], String> {
    let network_id = Sha256::digest(passphrase.as_bytes());
    let payload = xdr::TransactionSignaturePayload {
        network_id: xdr::Hash(network_id.into()),
        tagged_transaction: xdr::TransactionSignaturePayloadTaggedTransaction::Tx(tx.clone()),
    };
    let bytes = payload.to_xdr(Limits::none()).map_err(err)?;
    Ok(Sha256::digest(bytes).into())
}

/// Signs `tx` with `key`, producing the decorated signature to attach.
pub fn decorated_signature(
    key: &SigningKey,
    tx: &xdr::Transaction,
    passphrase: &str,
) -> Result<xdr::DecoratedSignature, String> {
    let payload = signature_payload(tx, passphrase)?;
    let signature: Signature = key.sign(&payload);
    let verifying: VerifyingKey = key.verifying_key();
    let hint = xdr::SignatureHint(verifying.to_bytes()[..4].try_into().map_err(err)?);
    Ok(xdr::DecoratedSignature {
        hint,
        signature: xdr::Signature::try_from(signature.to_bytes().to_vec()).map_err(err)?,
    })
}

/// Signs `tx` for `passphrase` from an `S...` secret, returning the base64-XDR
/// V1 envelope.
pub fn sign_envelope(
    tx: &xdr::Transaction,
    secret: &str,
    passphrase: &str,
) -> Result<String, String> {
    let (_, signing_key) = keypair_from_secret(secret)?;
    let signature = decorated_signature(&signing_key, tx, passphrase)?;
    let envelope = xdr::TransactionEnvelope::Tx(xdr::TransactionV1Envelope {
        tx: tx.clone(),
        signatures: vecm(vec![signature])?,
    });
    envelope.to_xdr_base64(Limits::none()).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::ReadXdr;

    const PASSPHRASE: &str = "Test SDF Network ; September 2015";

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[11u8; 32])
    }

    fn secret_strkey(key: &SigningKey) -> String {
        format!("{}", stellar_strkey::ed25519::PrivateKey(key.to_bytes()))
    }

    /// A minimal transaction good enough to sign.
    fn transaction() -> xdr::Transaction {
        xdr::Transaction {
            source_account: xdr::MuxedAccount::Ed25519(xdr::Uint256([1u8; 32])),
            fee: 100,
            seq_num: xdr::SequenceNumber(1),
            cond: xdr::Preconditions::None,
            memo: xdr::Memo::None,
            operations: vecm(vec![xdr::Operation {
                source_account: None,
                body: xdr::OperationBody::RestoreFootprint(xdr::RestoreFootprintOp {
                    ext: xdr::ExtensionPoint::V0,
                }),
            }])
            .unwrap(),
            ext: xdr::TransactionExt::V0,
        }
    }

    #[test]
    fn secret_roundtrips_to_the_same_account() {
        let key = signing_key();
        let account = keypair_from_secret(&secret_strkey(&key)).unwrap().0;
        assert_eq!(account_from_secret(&secret_strkey(&key)).unwrap(), account);
    }

    #[test]
    fn rejects_non_secret_strkey() {
        let public = stellar_strkey::ed25519::PublicKey([1u8; 32]).to_string();
        assert!(keypair_from_secret(&public).is_err());
        assert!(keypair_from_secret("nope").is_err());
    }

    #[test]
    fn payload_depends_on_the_passphrase() {
        let tx = transaction();
        let a = signature_payload(&tx, PASSPHRASE).unwrap();
        let b = signature_payload(&tx, "Public Global Stellar Network ; September 2015").unwrap();
        assert_ne!(a, b, "network id must be part of the payload");
    }

    #[test]
    fn envelope_has_one_signature_with_the_right_hint() {
        let key = signing_key();
        let tx = transaction();
        let b64 = sign_envelope(&tx, &secret_strkey(&key), PASSPHRASE).unwrap();
        let envelope = match xdr::TransactionEnvelope::from_xdr_base64(&b64, Limits::none())
            .expect("envelope decodes")
        {
            xdr::TransactionEnvelope::Tx(e) => e,
            other => panic!("expected a V1 envelope, got {other:?}"),
        };
        assert_eq!(envelope.tx.seq_num, tx.seq_num);
        assert_eq!(envelope.signatures.len(), 1);
        let expected_hint = &key.verifying_key().to_bytes()[..4];
        assert_eq!(&envelope.signatures[0].hint.0, expected_hint);
        assert_eq!(envelope.signatures[0].signature.0.len(), 64);
    }
}
