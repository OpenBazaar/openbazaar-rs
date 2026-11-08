//! The encodings of home node protocol version 2, as specified in
//! `docs/home-node-protocol.md`: keys and ids, pairing words, the pairing
//! onion, the pairing transcript and its emoji, request preimages, and the
//! LAN channel. `vectors.json` beside this file pins each one for the
//! TypeScript client.

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{schnorr, Message, Secp256k1, XOnlyPublicKey};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use p256::ecdsa::signature::Verifier;
use sha2::Sha256;
use x25519_dalek::{PublicKey as XPublic, StaticSecret};

use super::auth::decode_hex;
use crate::vault_key::encode_hex;

pub(crate) const SECRET_LEN: usize = 10;
pub(crate) const WORDS: usize = 8;
const LAN_LABEL: &[u8] = b"OBLAN1";
/// Response envelopes only. A request still uses [`LAN_LABEL`]: each request
/// key is used for one plaintext, so its fixed nonce does not repeat a
/// different message. The response key is reused for every answer to a
/// captured request, including errors, so each answer carries its own nonce.
const LAN_RESPONSE_LABEL: &[u8] = b"OBLAN2";
const LAN_RESPONSE_VERSION: u8 = 2;
const RESPONSE_NONCE_LEN: usize = 24;
/// Pinned by `vectors.json`. Production responses do not use this nonce.
#[cfg(test)]
pub(crate) const LAN_VECTOR_RESPONSE_NONCE: [u8; RESPONSE_NONCE_LEN] = [
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
];

pub(crate) fn node_id(node_public: &[u8; 32]) -> String {
    let mut text = b"OBNODE2".to_vec();
    text.extend_from_slice(node_public);
    encode_hex(&sha256::Hash::hash(&text).to_byte_array()[..16])
}

/// A device's public key, written `p256:{sec1 compressed hex}` or `bip340:{x-only hex}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeviceKey {
    P256(p256::ecdsa::VerifyingKey),
    Bip340(XOnlyPublicKey),
}

impl DeviceKey {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (alg, hex) = text.split_once(':')?;
        let bytes = decode_hex(hex)?;
        match alg {
            "p256" if bytes.len() == 33 => p256::ecdsa::VerifyingKey::from_sec1_bytes(&bytes)
                .ok()
                .map(Self::P256),
            "bip340" if bytes.len() == 32 => {
                XOnlyPublicKey::from_slice(&bytes).ok().map(Self::Bip340)
            }
            _ => None,
        }
    }

    pub(crate) fn text(&self) -> String {
        match self {
            Self::P256(key) => {
                format!("p256:{}", encode_hex(key.to_encoded_point(true).as_bytes()))
            }
            Self::Bip340(key) => format!("bip340:{}", encode_hex(&key.serialize())),
        }
    }

    pub(crate) fn id(&self) -> String {
        encode_hex(&sha256::Hash::hash(self.text().as_bytes()).to_byte_array()[..8])
    }

    /// Check `signature_hex` over `preimage` (SHA-256 for BIP 340; ECDSA-SHA256 DER for P-256).
    pub(crate) fn verifies(&self, preimage: &[u8], signature_hex: &str) -> bool {
        let Some(bytes) = decode_hex(signature_hex) else {
            return false;
        };
        match self {
            Self::P256(key) => p256::ecdsa::Signature::from_der(&bytes)
                .is_ok_and(|signature| key.verify(preimage, &signature).is_ok()),
            Self::Bip340(key) => schnorr::Signature::from_slice(&bytes).is_ok_and(|signature| {
                let digest = sha256::Hash::hash(preimage).to_byte_array();
                Secp256k1::verification_only()
                    .verify_schnorr(&signature, &Message::from_digest(digest), key)
                    .is_ok()
            }),
        }
    }
}

pub(crate) fn request_preimage(
    node_id: &str,
    device: &str,
    time: u64,
    nonce: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> String {
    let body_hash = encode_hex(&sha256::Hash::hash(body).to_byte_array());
    format!("OBRQ2\n{node_id}\n{device}\n{time}\n{nonce}\n{method}\n{path}\n{body_hash}")
}

/// The 8 BIP 39 words for a pairing secret: its 80 bits, then 8 bits of SHA-256.
pub(crate) fn words(secret: &[u8; SECRET_LEN]) -> Vec<&'static str> {
    let list = bip39::Language::English.word_list();
    let mut bits: u128 = 0;
    for byte in secret
        .iter()
        .chain(&sha256::Hash::hash(secret).to_byte_array()[..1])
    {
        bits = (bits << 8) | u128::from(*byte);
    }
    (0..WORDS)
        .rev()
        .map(|i| list[((bits >> (i * 11)) & 0x7ff) as usize])
        .collect()
}

/// The secret in typed words. Words match by their first four letters; the
/// checksum must hold.
pub(crate) fn parse_words(text: &str) -> Option<[u8; SECRET_LEN]> {
    let list = bip39::Language::English.word_list();
    let typed: Vec<String> = text
        .split(|ch: char| ch.is_whitespace() || ch == '-' || ch == ',' || ch == '.')
        .filter(|word| !word.is_empty())
        .map(|word| word.to_lowercase())
        .collect();
    if typed.len() != WORDS {
        return None;
    }
    let mut bits: u128 = 0;
    for word in &typed {
        let stem: String = word.chars().take(4).collect();
        let index = list.iter().position(|candidate| {
            candidate == word || (stem.len() == 4 && candidate.starts_with(&stem))
        })?;
        bits = (bits << 11) | index as u128;
    }
    let checksum = (bits & 0xff) as u8;
    let mut secret = [0u8; SECRET_LEN];
    for (i, byte) in secret.iter_mut().enumerate() {
        *byte = (bits >> (8 + 8 * (SECRET_LEN - 1 - i))) as u8;
    }
    (sha256::Hash::hash(&secret).to_byte_array()[0] == checksum).then_some(secret)
}

/// The ed25519 secret of the pairing onion for `secret`.
pub(crate) fn pairing_onion_seed(secret: &[u8; SECRET_LEN]) -> [u8; 32] {
    let mut seed = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(Some(b"OBPAIR2"), secret)
        .expand(b"onion", &mut seed)
        .expect("32 bytes is a valid HKDF length");
    seed
}

pub(crate) fn pairing_onion(secret: &[u8; SECRET_LEN]) -> String {
    tormail::onion_for_seed(&pairing_onion_seed(secret)).to_string()
}

/// The fields of `POST /pair/request` that the proof and signature cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PairRequest {
    pub key: String,
    pub watch_key: Option<String>,
    pub discovery_key: Option<String>,
    pub name: String,
    pub platform: String,
}

impl PairRequest {
    pub(crate) fn transcript(&self) -> String {
        format!(
            "OBPAIR2\n{}\n{}\n{}\n{}\n{}",
            self.key,
            self.watch_key.as_deref().unwrap_or(""),
            self.discovery_key.as_deref().unwrap_or(""),
            self.name,
            self.platform
        )
    }

    pub(crate) fn proof(&self, secret: &[u8; SECRET_LEN]) -> String {
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
        mac.update(self.transcript().as_bytes());
        encode_hex(&mac.finalize().into_bytes())
    }

    pub(crate) fn proves(&self, secret: &[u8; SECRET_LEN], proof_hex: &str) -> bool {
        let Some(proof) = decode_hex(proof_hex) else {
            return false;
        };
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
        mac.update(self.transcript().as_bytes());
        mac.verify_slice(&proof).is_ok()
    }

    /// The four emoji both screens show, with their names.
    pub(crate) fn sas(
        &self,
        secret: &[u8; SECRET_LEN],
        node_public: &[u8; 32],
    ) -> [(&'static str, &'static str); 4] {
        let mut text = b"OBSAS2\n".to_vec();
        text.extend_from_slice(secret);
        text.extend_from_slice(node_public);
        text.extend_from_slice(self.transcript().as_bytes());
        let hash = sha256::Hash::hash(&text).to_byte_array();
        std::array::from_fn(|i| SAS_EMOJI[usize::from(hash[i] & 63)])
    }
}

pub(crate) const SAS_EMOJI: [(&str, &str); 64] = [
    ("🐶", "dog"),
    ("🐱", "cat"),
    ("🦊", "fox"),
    ("🐻", "bear"),
    ("🐼", "panda"),
    ("🐨", "koala"),
    ("🐯", "tiger"),
    ("🦁", "lion"),
    ("🐮", "cow"),
    ("🐷", "pig"),
    ("🐸", "frog"),
    ("🐵", "monkey"),
    ("🐔", "chicken"),
    ("🐧", "penguin"),
    ("🐦", "bird"),
    ("🦉", "owl"),
    ("🦋", "butterfly"),
    ("🐢", "turtle"),
    ("🐙", "octopus"),
    ("🦀", "crab"),
    ("🐳", "whale"),
    ("🐬", "dolphin"),
    ("🐟", "fish"),
    ("🦈", "shark"),
    ("🌵", "cactus"),
    ("🌲", "tree"),
    ("🌻", "sunflower"),
    ("🍄", "mushroom"),
    ("🌙", "moon"),
    ("⭐", "star"),
    ("☀️", "sun"),
    ("⚡", "lightning"),
    ("🔥", "fire"),
    ("💧", "droplet"),
    ("🌈", "rainbow"),
    ("❄️", "snowflake"),
    ("🍎", "apple"),
    ("🍋", "lemon"),
    ("🍌", "banana"),
    ("🍇", "grapes"),
    ("🍓", "strawberry"),
    ("🥕", "carrot"),
    ("🌽", "corn"),
    ("🍕", "pizza"),
    ("🎂", "cake"),
    ("☕", "coffee"),
    ("🚲", "bicycle"),
    ("🚗", "car"),
    ("🚀", "rocket"),
    ("⛵", "sailboat"),
    ("🎈", "balloon"),
    ("🎁", "gift"),
    ("🔑", "key"),
    ("🔔", "bell"),
    ("🎸", "guitar"),
    ("📚", "books"),
    ("⚓", "anchor"),
    ("🧭", "compass"),
    ("⏰", "clock"),
    ("💡", "bulb"),
    ("🔒", "lock"),
    ("✂️", "scissors"),
    ("🧩", "puzzle"),
    ("👑", "crown"),
];

fn lan_keys(
    shared: &[u8; 32],
    ephemeral: &[u8; 32],
    node_public: &[u8; 32],
) -> ([u8; 32], [u8; 32]) {
    let mut info = ephemeral.to_vec();
    info.extend_from_slice(node_public);
    let mut okm = [0u8; 64];
    hkdf::Hkdf::<Sha256>::new(Some(LAN_LABEL), shared)
        .expand(&info, &mut okm)
        .expect("64 bytes is a valid HKDF length");
    let (request, response) = okm.split_at(32);
    (
        request.try_into().expect("32 bytes"),
        response.try_into().expect("32 bytes"),
    )
}

fn seal(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(
            Nonce::from_slice(&[0; 12]),
            Payload {
                msg: plaintext,
                aad: LAN_LABEL,
            },
        )
        .expect("ChaCha20-Poly1305 seals any length")
}

fn open(key: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(
            Nonce::from_slice(&[0; 12]),
            Payload {
                msg: ciphertext,
                aad: LAN_LABEL,
            },
        )
        .ok()
}

/// Node side: the inner request in a `POST /lan` body, and the key for the answer.
pub(crate) fn lan_open_request(node: &StaticSecret, body: &[u8]) -> Option<(Vec<u8>, [u8; 32])> {
    let ephemeral: [u8; 32] = body.get(..32)?.try_into().ok()?;
    let shared = node.diffie_hellman(&XPublic::from(ephemeral));
    if !shared.was_contributory() {
        return None;
    }
    let (request_key, response_key) = lan_keys(
        shared.as_bytes(),
        &ephemeral,
        XPublic::from(node).as_bytes(),
    );
    Some((open(&request_key, &body[32..])?, response_key))
}

/// `0x02 ‖ nonce ‖ XChaCha20-Poly1305(Kres, nonce, aad = "OBLAN2", answer)`.
/// `nonce` must be 24 bytes and not all zeros. A caller that repeats a nonce
/// under one response key reopens the captured-envelope attack.
pub(crate) fn lan_seal_response_with_nonce(
    response_key: &[u8; 32],
    answer: &[u8],
    nonce: &[u8; RESPONSE_NONCE_LEN],
) -> Vec<u8> {
    assert!(
        *nonce != [0; RESPONSE_NONCE_LEN],
        "a LAN response nonce must not be all zeros"
    );
    let ciphertext = XChaCha20Poly1305::new(Key::from_slice(response_key))
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: answer,
                aad: LAN_RESPONSE_LABEL,
            },
        )
        .expect("XChaCha20-Poly1305 seals any length");
    let mut body = Vec::with_capacity(1 + RESPONSE_NONCE_LEN + ciphertext.len());
    body.push(LAN_RESPONSE_VERSION);
    body.extend_from_slice(nonce);
    body.extend(ciphertext);
    body
}

/// The nonce in a version-2 response envelope.
pub(crate) fn lan_response_nonce(body: &[u8]) -> Option<[u8; RESPONSE_NONCE_LEN]> {
    if body.first().copied() != Some(LAN_RESPONSE_VERSION) || body.len() < 1 + RESPONSE_NONCE_LEN {
        return None;
    }
    let nonce: [u8; RESPONSE_NONCE_LEN] = body[1..1 + RESPONSE_NONCE_LEN].try_into().ok()?;
    (nonce != [0; RESPONSE_NONCE_LEN]).then_some(nonce)
}

/// Device side: a `POST /lan` body for `request`, and the key to open the answer.
pub(crate) fn lan_seal_request(
    node_public: &[u8; 32],
    ephemeral: &StaticSecret,
    request: &[u8],
) -> (Vec<u8>, [u8; 32]) {
    let ephemeral_public = *XPublic::from(ephemeral).as_bytes();
    let shared = ephemeral.diffie_hellman(&XPublic::from(*node_public));
    let (request_key, response_key) = lan_keys(shared.as_bytes(), &ephemeral_public, node_public);
    let mut body = ephemeral_public.to_vec();
    body.extend(seal(&request_key, request));
    (body, response_key)
}

pub(crate) fn lan_open_response(response_key: &[u8; 32], body: &[u8]) -> Option<Vec<u8>> {
    let nonce = lan_response_nonce(body)?;
    let ciphertext = body.get(1 + RESPONSE_NONCE_LEN..)?;
    if ciphertext.len() < 16 {
        return None;
    }
    XChaCha20Poly1305::new(Key::from_slice(response_key))
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: ciphertext,
                aad: LAN_RESPONSE_LABEL,
            },
        )
        .ok()
}

pub(crate) fn base32(bytes: &[u8]) -> String {
    data_encoding::BASE32_NOPAD.encode(bytes).to_lowercase()
}

pub(crate) fn decode_base32(text: &str) -> Option<Vec<u8>> {
    data_encoding::BASE32_NOPAD
        .decode(text.trim().to_uppercase().as_bytes())
        .ok()
}
