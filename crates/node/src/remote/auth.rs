//! Signed requests from paired devices.
//!
//! A device holds a 32-byte secret; its public key is the BIP 340 x-only key
//! of that secret, and its id is the first 16 hex digits of SHA-256 of the
//! key. Every request carries four headers:
//!
//! | header | value |
//! |---|---|
//! | `X-Ob-Device` | device id |
//! | `X-Ob-Time` | unix seconds when signed |
//! | `X-Ob-Nonce` | 32 random hex digits |
//! | `X-Ob-Signature` | BIP 340 signature, 128 hex digits |
//!
//! The signature covers SHA-256 of
//! `OBRQ1\n{nodeId}\n{deviceId}\n{time}\n{nonce}\n{METHOD}\n{path}\n{sha256(body) hex}`,
//! so a request cannot be moved to another node, method, path, or body.
//! The node takes a time within [`WINDOW_SECS`] of its clock and not before
//! it started, and each nonce once per device. Nonces are kept in memory
//! for twice the window; a restart forgets them, which is why a time from
//! before the restart is refused.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{schnorr, Keypair, Message, Secp256k1, XOnlyPublicKey};

use crate::http_parse::header;
use crate::vault_key::encode_hex;

pub(crate) const WINDOW_SECS: u64 = 120;
const MAX_NONCES_PER_DEVICE: usize = 4096;
pub(crate) const DEVICE_HEADER: &str = "X-Ob-Device";
pub(crate) const TIME_HEADER: &str = "X-Ob-Time";
pub(crate) const NONCE_HEADER: &str = "X-Ob-Nonce";
pub(crate) const SIGNATURE_HEADER: &str = "X-Ob-Signature";

pub(crate) fn device_id(key: &XOnlyPublicKey) -> String {
    encode_hex(&sha256::Hash::hash(&key.serialize()).to_byte_array()[..8])
}

pub(crate) fn request_digest(node_id: &str, device: &str, time: u64, nonce: &str, method: &str, path: &str, body: &[u8]) -> [u8; 32] {
    let body_hash = encode_hex(&sha256::Hash::hash(body).to_byte_array());
    let text = format!("OBRQ1\n{node_id}\n{device}\n{time}\n{nonce}\n{method}\n{path}\n{body_hash}");
    sha256::Hash::hash(text.as_bytes()).to_byte_array()
}

pub(crate) fn pair_digest(secret: &str, key_hex: &str, name: &str) -> [u8; 32] {
    sha256::Hash::hash(format!("OBPAIR1\n{secret}\n{key_hex}\n{name}").as_bytes()).to_byte_array()
}

pub(crate) fn verify(key: &XOnlyPublicKey, digest: [u8; 32], signature_hex: &str) -> bool {
    let Some(bytes) = decode_hex(signature_hex) else {
        return false;
    };
    let Ok(signature) = schnorr::Signature::from_slice(&bytes) else {
        return false;
    };
    Secp256k1::verification_only()
        .verify_schnorr(&signature, &Message::from_digest(digest), key)
        .is_ok()
}

pub(crate) fn parse_key(hex: &str) -> Option<XOnlyPublicKey> {
    XOnlyPublicKey::from_slice(&decode_hex(hex)?).ok()
}

pub(crate) fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

/// A device's signing key, held by a node acting as the remote client.
pub(crate) struct DeviceKey {
    keypair: Keypair,
}

impl DeviceKey {
    pub(crate) fn from_secret(secret: &[u8; 32]) -> Option<Self> {
        Keypair::from_seckey_slice(&Secp256k1::new(), secret).ok().map(|keypair| Self { keypair })
    }

    pub(crate) fn public(&self) -> XOnlyPublicKey {
        self.keypair.x_only_public_key().0
    }

    pub(crate) fn id(&self) -> String {
        device_id(&self.public())
    }

    pub(crate) fn sign(&self, digest: [u8; 32]) -> String {
        let aux = crate::vault_key::random_bytes::<32>();
        let signature = Secp256k1::new().sign_schnorr_with_aux_rand(&Message::from_digest(digest), &self.keypair, &aux);
        encode_hex(signature.as_ref())
    }

    /// The four auth headers for one request, each ending in CRLF.
    pub(crate) fn headers(&self, node_id: &str, time: u64, method: &str, path: &str, body: &[u8]) -> String {
        let nonce = encode_hex(&crate::vault_key::random_bytes::<16>());
        let id = self.id();
        let signature = self.sign(request_digest(node_id, &id, time, &nonce, method, path, body));
        format!("{DEVICE_HEADER}: {id}\r\n{TIME_HEADER}: {time}\r\n{NONCE_HEADER}: {nonce}\r\n{SIGNATURE_HEADER}: {signature}\r\n")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Outside the window, or before this process started.
    Clock,
    Replay,
}

/// The signed headers of one request, before the key is looked up.
pub(crate) struct Signed<'a> {
    pub device: &'a str,
    time: u64,
    nonce: &'a str,
    signature: &'a str,
}

impl<'a> Signed<'a> {
    pub(crate) fn read(head: &'a str) -> Option<Self> {
        let device = header(head, DEVICE_HEADER)?;
        let time = header(head, TIME_HEADER)?.parse().ok()?;
        let nonce = header(head, NONCE_HEADER)?;
        let signature = header(head, SIGNATURE_HEADER)?;
        let ok = device.len() == 16
            && device.bytes().all(|b| b.is_ascii_hexdigit())
            && nonce.len() == 32
            && nonce.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then_some(Self { device, time, nonce, signature })
    }

    pub(crate) fn verifies(&self, key: &XOnlyPublicKey, node_id: &str, method: &str, path: &str, body: &[u8]) -> bool {
        verify(key, request_digest(node_id, self.device, self.time, self.nonce, method, path, body), self.signature)
    }
}

/// Times and nonces already used, per device.
pub(crate) struct ReplayGuard {
    started: u64,
    seen: Mutex<HashMap<String, VecDeque<(u64, String)>>>,
}

impl ReplayGuard {
    pub(crate) fn new(started: u64) -> Self {
        Self { started, seen: Mutex::new(HashMap::new()) }
    }

    /// Record the nonce of a request whose signature already verified.
    pub(crate) fn admit(&self, signed: &Signed<'_>, now: u64) -> Result<(), Refused> {
        if signed.time < self.started || signed.time + WINDOW_SECS < now || signed.time > now + WINDOW_SECS {
            return Err(Refused::Clock);
        }
        let mut seen = self.seen.lock().unwrap_or_else(|err| err.into_inner());
        let list = seen.entry(signed.device.to_string()).or_default();
        while list.front().is_some_and(|(time, _)| time + 2 * WINDOW_SECS < now) {
            list.pop_front();
        }
        if list.iter().any(|(_, nonce)| nonce == signed.nonce) {
            return Err(Refused::Replay);
        }
        if list.len() >= MAX_NONCES_PER_DEVICE {
            return Err(Refused::Replay);
        }
        list.push_back((now.max(signed.time), signed.nonce.to_string()));
        Ok(())
    }

    pub(crate) fn forget(&self, device: &str) {
        self.seen.lock().unwrap_or_else(|err| err.into_inner()).remove(device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The same vector is checked in apps/mobile/src/homeNode/signing.test.ts.
    #[test]
    fn digests_and_signatures_match_the_web_client() {
        let key = DeviceKey::from_secret(&[7; 32]).unwrap();
        let public = key.public().to_string();
        assert_eq!(public, "989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f");
        assert_eq!(key.id(), "3f662275b53a056c");
        let digest = request_digest(
            "00112233445566778899aabbccddeeff",
            &key.id(),
            1_700_000_000,
            "0123456789abcdef0123456789abcdef",
            "POST",
            "/listings?x=1",
            br#"{"a":1}"#,
        );
        assert_eq!(encode_hex(&digest), "5e8bcb08b73a1aa2f6d4d6988896ba02356f97ea7263da63045c5e39d22c1e77");
        assert_eq!(
            encode_hex(&pair_digest("abcdefghijklmnopqrstuvwxyz234567", &public, "Phone")),
            "7a2dcd64d847362dfd278ef8eddab148d4385f18a012e60cdb3adab4ababa66c"
        );
        let from_web = "dfe33293c18658c987a5782edc28756069e20e87c1d8eaecc78975f1cfe689e77eafbfc6175228006dcd6e6d850128130789508a30fefe900f1eaae560314701";
        assert!(verify(&key.public(), digest, from_web));
        assert!(verify(&key.public(), digest, &key.sign(digest)));
        assert!(!verify(&key.public(), [0; 32], from_web));
    }
}
