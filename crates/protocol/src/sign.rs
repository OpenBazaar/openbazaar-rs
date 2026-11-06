use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::schnorr;
use bitcoin::secp256k1::{Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};

use crate::Error;

pub const PUBKEY_LEN: usize = 32;
pub const SIG_LEN: usize = 64;

/// BIP-340 tagged hash: SHA256(SHA256(tag) || SHA256(tag) || msg).
pub fn tagged_hash(tag: &str, body: &[u8]) -> [u8; 32] {
    let tag_hash = sha256::Hash::hash(tag.as_bytes());
    let tag_bytes = tag_hash.as_byte_array();
    sha256::Hash::hash_byte_chunks([tag_bytes.as_slice(), tag_bytes.as_slice(), body])
        .to_byte_array()
}

pub fn content_hash(body: &[u8]) -> [u8; 32] {
    sha256::Hash::hash(body).to_byte_array()
}

/// A fixture or test scalar. Not a backup phrase and not a derivation.
pub fn secret_from_label(label: &str) -> SecretKey {
    let digest = content_hash(label.as_bytes());
    SecretKey::from_slice(&digest)
        .unwrap_or_else(|_| panic!("label {label:?} does not hash to a valid secret"))
}

pub fn xonly(secret: &SecretKey) -> [u8; PUBKEY_LEN] {
    let secp = Secp256k1::new();
    let keypair = Keypair::from_secret_key(&secp, secret);
    keypair.x_only_public_key().0.serialize()
}

pub fn sign(tag: &str, body: &[u8], secret: &SecretKey) -> [u8; SIG_LEN] {
    let secp = Secp256k1::new();
    let keypair = Keypair::from_secret_key(&secp, secret);
    let message = Message::from_digest(tagged_hash(tag, body));
    secp.sign_schnorr_no_aux_rand(&message, &keypair)
        .serialize()
}

pub fn verify(
    tag: &str,
    body: &[u8],
    pubkey: &[u8; PUBKEY_LEN],
    signature: &[u8; SIG_LEN],
) -> Result<(), Error> {
    let secp = Secp256k1::new();
    let pubkey = XOnlyPublicKey::from_slice(pubkey).map_err(|_| Error::BadKey)?;
    let signature = schnorr::Signature::from_slice(signature).map_err(|_| Error::BadSignature)?;
    let message = Message::from_digest(tagged_hash(tag, body));
    secp.verify_schnorr(&signature, &message, &pubkey)
        .map_err(|_| Error::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schnorr_round_trip_rejects_tampering() {
        let secret = secret_from_label("sign-test");
        let body = b"hello";
        let signature = sign("ob/test", body, &secret);
        let pubkey = xonly(&secret);
        verify("ob/test", body, &pubkey, &signature).unwrap();

        let mut changed = *body;
        changed[0] ^= 0xff;
        assert!(verify("ob/test", &changed, &pubkey, &signature).is_err());
        assert!(verify("ob/other", body, &pubkey, &signature).is_err());

        let mut bad_sig = signature;
        bad_sig[0] ^= 0x01;
        assert!(verify("ob/test", body, &pubkey, &bad_sig).is_err());
    }
}
