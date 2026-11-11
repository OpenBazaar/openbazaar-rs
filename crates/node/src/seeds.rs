//! The seed-document key and the two introduction onions shipped in this build.
//!
//! Peers accept a directory head only when it verifies under [`DOCUMENT_KEY`].
//! The secret that signs heads stays off the hosts and out of this tree.
//! [`INTRODUCTIONS`] are the bootstrap: a node with no saved head dials them.
//! After a head is stored, the node dials the list in that head. The private
//! keys for the compiled onions live in `data/intro-a` and `data/intro-b` on
//! the operator machine (`data/` is gitignored).

use openbazaar_protocol::{DirectoryTerms, SeedDocument};

/// BIP-340 x-only public key for the testnet4 directory head.
/// The matching secret is `data/seed-document.key` on the operator laptop.
/// That file matches `*.key` in `.gitignore` and is not on the introduction hosts.
pub const DOCUMENT_KEY: [u8; 32] = [
    0xaa, 0x8c, 0xab, 0x28, 0x20, 0xfa, 0x74, 0x0a, 0x09, 0x85, 0x08, 0xde, 0xa7, 0xc1, 0xad, 0x0a,
    0xf3, 0x51, 0xc2, 0x4f, 0xf1, 0x21, 0x88, 0x71, 0x94, 0x6f, 0x77, 0xb4, 0x97, 0x60, 0x15, 0xc0,
];

/// Sign a directory head with the operator secret. Refuses a secret whose
/// public key is not [`DOCUMENT_KEY`]. `introductions` is the list the head
/// publishes. An empty list is the two compiled onions, which is the first head.
/// The fee is 20,000 sats, 1 confirmation, 2,016 blocks, paid to [`DOCUMENT_KEY`].
pub fn sign_document(sequence: u64, introductions: Vec<String>, secret_bytes: [u8; 32]) -> Result<Vec<u8>, String> {
    use bitcoin::secp256k1::{Secp256k1, SecretKey};

    let secret = SecretKey::from_slice(&secret_bytes).map_err(|_| "the document key is not a secret key".to_string())?;
    let public = secret.x_only_public_key(&Secp256k1::new()).0.serialize();
    if public != DOCUMENT_KEY {
        return Err("this key does not match the public key shipped in the binary".into());
    }
    let introductions = if introductions.is_empty() {
        INTRODUCTIONS.iter().map(|onion| (*onion).to_string()).collect()
    } else {
        introductions
    };
    let document = SeedDocument::sign(sequence, introductions, DirectoryTerms::standard(DOCUMENT_KEY), None, &secret)
        .map_err(|err| err.to_string())?;
    document.encode().map_err(|err| err.to_string())
}

pub const INTRODUCTIONS: [&str; 2] = [
    "a5kon2xfzc2myexkxvvrdcw73jare5ptov4cyu3zp6alfcthge7ilmqd.onion",
    "z2arrch7sx44bkuz54s5m2ardeopml2xdbuifun6ubhxgocac53jbxid.onion",
];

/// Up to three hosts for one pass, starting at `origin` and wrapping.
pub fn pass_hosts(hosts: &[String], origin: usize) -> Vec<String> {
    if hosts.is_empty() {
        return Vec::new();
    }
    let start = origin % hosts.len();
    (0..3.min(hosts.len())).map(|step| hosts[(start + step) % hosts.len()].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::{pass_hosts, DOCUMENT_KEY, INTRODUCTIONS};
    use openbazaar_protocol::{DirectoryTerms, SeedDocument, FEE_CONFS, FEE_SATS, EPOCH_BLOCKS};

    #[test]
    fn the_shipped_document_key_is_an_x_only_point() {
        bitcoin::secp256k1::XOnlyPublicKey::from_slice(&DOCUMENT_KEY).expect("document key");
    }

    #[test]
    fn a_different_secret_cannot_sign_and_the_operator_key_can() {
        use openbazaar_protocol::secret_from_label;

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
        let document = SeedDocument::decode(&bytes).unwrap();
        document.verify_signer(&DOCUMENT_KEY).unwrap();
        assert_eq!(document.terms, DirectoryTerms::standard(DOCUMENT_KEY));
        assert_eq!((document.terms.fee_sats, document.terms.fee_confs, document.terms.epoch_blocks), (FEE_SATS, FEE_CONFS, EPOCH_BLOCKS));
        assert_eq!(document.introductions.len(), INTRODUCTIONS.len());
    }

    #[test]
    fn sign_document_uses_the_list_it_is_given() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/seed-document.key");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let mut secret = [0u8; 32];
        for (index, byte) in secret.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text.trim()[index * 2..index * 2 + 2], 16).unwrap();
        }
        let third = format!("{}.onion", "c".repeat(56));
        let bytes = super::sign_document(2, vec![third.clone()], secret).unwrap();
        let document = SeedDocument::decode(&bytes).unwrap();
        assert_eq!(document.introductions, vec![third]);
        assert_eq!(document.sequence, 2);
    }

    #[test]
    fn a_pass_asks_three_hosts_and_wraps() {
        let hosts: Vec<String> = (0..5).map(|n| format!("host-{n}")).collect();
        assert_eq!(pass_hosts(&hosts, 0), vec!["host-0", "host-1", "host-2"]);
        assert_eq!(pass_hosts(&hosts, 4), vec!["host-4", "host-0", "host-1"]);
        assert_eq!(pass_hosts(&hosts[..2], 1).len(), 2);
    }
}
