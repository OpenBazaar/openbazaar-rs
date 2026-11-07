//! Lightning key material from one seed-derived secret.
//!
//! The node crate derives a 32-byte secret from the wallet phrase on the
//! Lightning path (`m/1984'/coin'/6'/0'`). Two keys come from it, each with
//! its own HMAC-SHA512 label, so neither can be computed from the other:
//!
//! | key | label | use |
//! |---|---|---|
//! | LDK entropy (64 bytes) | `OpenBazaar Lightning seed v1` | `ldk_node::Builder::set_entropy_seed_bytes`: node id, channel keys, LDK's on-chain wallet |
//! | store key (32 bytes) | `OpenBazaar Lightning store v1` | seals every value LDK persists |
//!
//! Restoring the phrase gives back the node id and the on-chain wallet, not
//! channel state: channels need the sealed store (or the LSP's cooperation).

use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::Zeroizing;

const SEED_LABEL: &[u8] = b"OpenBazaar Lightning seed v1";
const STORE_LABEL: &[u8] = b"OpenBazaar Lightning store v1";

fn hmac(label: &[u8], secret: &[u8; 32]) -> Zeroizing<[u8; 64]> {
    let mut mac = Hmac::<Sha512>::new_from_slice(label).expect("HMAC takes any key length");
    mac.update(secret);
    let mut out = Zeroizing::new([0u8; 64]);
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// The 64 bytes LDK derives every Lightning key from.
pub fn ldk_entropy(secret: &[u8; 32]) -> Zeroizing<[u8; 64]> {
    hmac(SEED_LABEL, secret)
}

/// The key that seals LDK's persisted state.
pub fn store_key(secret: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let wide = hmac(STORE_LABEL, secret);
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&wide[..32]);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_separate_the_keys_and_derivation_is_stable() {
        let secret = [7u8; 32];
        let entropy = ldk_entropy(&secret);
        let store = store_key(&secret);
        assert_eq!(*entropy, *ldk_entropy(&secret));
        assert_ne!(&entropy[..32], &store[..]);
        assert_ne!(*ldk_entropy(&[8u8; 32]), *entropy);
        assert_eq!(
            hex::encode(&store[..]),
            hex::encode(&store_key(&[7u8; 32])[..]),
        );
    }
}
