//! The seed-document key and the two introduction onions shipped in this build.
//!
//! Peers accept a seed document only when it verifies under [`DOCUMENT_KEY`].
//! The secret that signs documents stays off the hosts and out of this tree.
//! [`INTRODUCTIONS`] are the two onions a peer asks. A document is kept when
//! either one returns a higher sequence that verifies. The private keys for
//! these onions live in `data/intro-a` and `data/intro-b` on the operator
//! machine (`data/` is gitignored). Replacing either onion is a new build.

/// BIP-340 x-only public key for the testnet4 seed document.
/// The matching secret is `data/seed-document.key` on the operator laptop.
/// That file matches `*.key` in `.gitignore` and is not on the introduction hosts.
pub const DOCUMENT_KEY: [u8; 32] = [
    0xaa, 0x8c, 0xab, 0x28, 0x20, 0xfa, 0x74, 0x0a, 0x09, 0x85, 0x08, 0xde, 0xa7, 0xc1, 0xad, 0x0a,
    0xf3, 0x51, 0xc2, 0x4f, 0xf1, 0x21, 0x88, 0x71, 0x94, 0x6f, 0x77, 0xb4, 0x97, 0x60, 0x15, 0xc0,
];

/// Sign a seed document with the operator secret. Refuses a secret whose
/// public key is not [`DOCUMENT_KEY`], so a host cannot be given a different key.
pub fn sign_document(sequence: u64, stores: Vec<[u8; 32]>, secret_bytes: [u8; 32]) -> Result<Vec<u8>, String> {
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use openbazaar_protocol::SeedDocument;

    let secret = SecretKey::from_slice(&secret_bytes).map_err(|_| "the document key is not a secret key".to_string())?;
    let public = secret.x_only_public_key(&Secp256k1::new()).0.serialize();
    if public != DOCUMENT_KEY {
        return Err("this key does not match the public key shipped in the binary".into());
    }
    let introductions = INTRODUCTIONS.iter().map(|onion| (*onion).to_string()).collect();
    let document = SeedDocument::sign(sequence, introductions, stores, None, &secret).map_err(|err| err.to_string())?;
    document.encode().map_err(|err| err.to_string())
}

pub const INTRODUCTIONS: [&str; 2] = [
    "a5kon2xfzc2myexkxvvrdcw73jare5ptov4cyu3zp6alfcthge7ilmqd.onion",
    "z2arrch7sx44bkuz54s5m2ardeopml2xdbuifun6ubhxgocac53jbxid.onion",
];

#[cfg(test)]
mod tests {
    use super::DOCUMENT_KEY;

    #[test]
    fn the_shipped_document_key_is_an_x_only_point() {
        bitcoin::secp256k1::XOnlyPublicKey::from_slice(&DOCUMENT_KEY).expect("document key");
    }

    #[test]
    fn a_different_secret_cannot_sign_and_the_operator_key_can() {
        use openbazaar_protocol::{secret_from_label, SeedDocument};

        let wrong = secret_from_label("not-the-operator").secret_bytes();
        let err = super::sign_document(1, vec![], wrong).unwrap_err();
        assert!(err.contains("does not match"), "{err}");

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/seed-document.key");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let mut secret = [0u8; 32];
        let text = text.trim();
        assert_eq!(text.len(), 64);
        for (index, byte) in secret.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
        }
        let bytes = super::sign_document(1, vec![], secret).unwrap();
        SeedDocument::decode(&bytes).unwrap().verify_signer(&DOCUMENT_KEY).unwrap();
    }
}
