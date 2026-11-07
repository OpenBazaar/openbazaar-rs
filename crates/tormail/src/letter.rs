//! The authenticated, padded plaintext inside every sealed letter (`OBLT` v1).
//!
//! ```text
//! OBLT | 0x01 | kind (u8) | sender (32) | message id (16) | body length (u32) | body
//!      | signature (64) | zero padding up to the size bucket
//! ```
//!
//! The signature is BIP 340 by the sender key over a tagged hash
//! (`ob/letter/v1`) of the recipient's X25519 key, the kind, the sender key,
//! the message id, and the body. HPKE then seals the whole padded plaintext
//! in base mode, so the sender key, the id, and the true length are all
//! inside the ciphertext.
//!
//! # Why a signature by a per-conversation key
//!
//! HPKE base mode lets anyone who knows a mailbox key write a letter that
//! opens. A letter could be authenticated three ways, and the choice decides
//! what links a buyer's orders:
//!
//! * a long-term buyer key (HPKE Auth with a static key, or a signature by
//!   an identity key) would let every vendor, and any two vendors together,
//!   link all of one buyer's orders. The paper rules this out.
//! * HPKE Auth with a fresh per-order X25519 key needs the recipient to know
//!   that key before it can open the letter, so the key would travel in the
//!   clear beside the ciphertext, where a mailbox or relay sees it on every
//!   letter of the order.
//! * a signature inside the ciphertext by a fresh key per conversation (one
//!   order, one escrow, one dispute) reveals nothing outside the ciphertext,
//!   and nothing that links two conversations: each key is derived from a
//!   root secret and the conversation id by a one-way hash.
//!
//! We use the third. What it gives: every letter after the first in a
//! conversation must come from the same key as the first (the recipient pins
//! it), so a third party who learns a mailbox key and a conversation id cannot
//! inject or alter letters in someone else's order or escrow; the signed
//! recipient key stops a vendor from re-sealing a buyer's signed letter to
//! someone else as if the buyer had sent it there; and the message id lets
//! the recipient refuse a replayed letter. What it cannot give: the first
//! letter of a new order is from an anonymous key by design, so a vendor
//! still treats a new order as an offer that becomes real when it is paid.
//! A vendor's reply is authenticated a step further: only the vendor read
//! the per-order reply key, so a reply sealed to it came from the vendor.

use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::secp256k1::{schnorr, Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::Error;

const MAGIC: &[u8; 4] = b"OBLT";
const VERSION: u8 = 1;
const SIGN_TAG: &[u8] = b"ob/letter/v1";
const DERIVE_TAG: &[u8] = b"ob/letter-key/v1";
const SIG_LEN: usize = 64;

/// Bytes the envelope adds around a body, before padding.
pub(crate) const OVERHEAD: usize = 4 + 1 + 1 + 32 + 16 + 4 + SIG_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Order = 1,
    Message = 2,
    Reply = 3,
}

/// A sender's signing key for one conversation (an order, an escrow, a dispute).
pub struct SenderKey(Keypair);

/// The x-only public half of a [`SenderKey`]. Recipients pin it per conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SenderId(pub [u8; 32]);

/// A random id per letter. Recipients refuse an id they have seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageId(pub [u8; 16]);

/// A letter that opened and whose signature verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened<T> {
    pub sender: SenderId,
    pub message_id: MessageId,
    pub body: T,
}

impl SenderKey {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        loop {
            StdRng::from_os_rng().fill_bytes(&mut bytes);
            if let Ok(key) = Self::from_secret_bytes(bytes) {
                return key;
            }
        }
    }

    pub fn from_secret_bytes(bytes: [u8; 32]) -> Result<Self, Error> {
        let secret = SecretKey::from_slice(&bytes).map_err(|_| Error::Malformed)?;
        Ok(Self(Keypair::from_secret_key(&Secp256k1::signing_only(), &secret)))
    }

    /// The key for one conversation, derived from `root` by a one-way hash.
    ///
    /// Two conversations get unrelated keys, so nothing a recipient sees
    /// links them, while the sender can derive the same key again from the
    /// conversation id alone.
    pub fn derive(root: &[u8; 32], purpose: &str, conversation: &[u8]) -> Self {
        let mut counter = 0u8;
        loop {
            if let Ok(key) = Self::from_secret_bytes(derive_secret(root, purpose, conversation, counter)) {
                return key;
            }
            counter += 1;
        }
    }

    pub fn id(&self) -> SenderId {
        SenderId(self.0.x_only_public_key().0.serialize())
    }
}

/// 32 secret bytes for one conversation and purpose, from `root` by a tagged
/// SHA-256. Without `root` the output is unpredictable; with it, repeatable.
pub fn derive_secret(root: &[u8; 32], purpose: &str, conversation: &[u8], counter: u8) -> [u8; 32] {
    let tag = sha256::Hash::hash(DERIVE_TAG);
    let mut engine = sha256::Hash::engine();
    engine.input(tag.as_byte_array());
    engine.input(tag.as_byte_array());
    engine.input(root);
    engine.input(&(purpose.len() as u16).to_le_bytes());
    engine.input(purpose.as_bytes());
    engine.input(conversation);
    engine.input(&[counter]);
    sha256::Hash::from_engine(engine).to_byte_array()
}

impl MessageId {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 16];
        StdRng::from_os_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }
}

fn digest(kind: Kind, recipient: &[u8; 32], sender: &SenderId, id: &MessageId, body: &[u8]) -> Message {
    let tag = sha256::Hash::hash(SIGN_TAG);
    let mut engine = sha256::Hash::engine();
    engine.input(tag.as_byte_array());
    engine.input(tag.as_byte_array());
    engine.input(recipient);
    engine.input(&[kind as u8]);
    engine.input(&sender.0);
    engine.input(&id.0);
    engine.input(&(body.len() as u32).to_le_bytes());
    engine.input(body);
    Message::from_digest(sha256::Hash::from_engine(engine).to_byte_array())
}

/// Sign `body` for `recipient` and pad the plaintext to exactly `padded_len` bytes.
pub(crate) fn encode(
    kind: Kind,
    recipient: &[u8; 32],
    from: &SenderKey,
    body: &[u8],
    padded_len: usize,
) -> Result<Vec<u8>, Error> {
    if OVERHEAD + body.len() > padded_len {
        return Err(Error::InvalidOrder("letter body does not fit its size bucket"));
    }
    let sender = from.id();
    let id = MessageId::generate();
    let signature = Secp256k1::signing_only().sign_schnorr_no_aux_rand(&digest(kind, recipient, &sender, &id, body), &from.0);
    let mut out = Vec::with_capacity(padded_len);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(kind as u8);
    out.extend_from_slice(&sender.0);
    out.extend_from_slice(&id.0);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(signature.as_ref());
    out.resize(padded_len, 0);
    Ok(out)
}

/// Check the envelope and its signature. The padding must be all zero.
pub(crate) fn decode(kind: Kind, recipient: &[u8; 32], plaintext: &[u8]) -> Result<Opened<Vec<u8>>, Error> {
    if plaintext.len() < OVERHEAD || &plaintext[..4] != MAGIC || plaintext[4] != VERSION {
        return Err(Error::Malformed);
    }
    if plaintext[5] != kind as u8 {
        return Err(Error::Malformed);
    }
    let sender = SenderId(plaintext[6..38].try_into().expect("32 bytes"));
    let id = MessageId(plaintext[38..54].try_into().expect("16 bytes"));
    let len = u32::from_le_bytes(plaintext[54..58].try_into().expect("4 bytes")) as usize;
    let body_end = 58usize.checked_add(len).ok_or(Error::Malformed)?;
    let sig_end = body_end.checked_add(SIG_LEN).ok_or(Error::Malformed)?;
    if sig_end > plaintext.len() || plaintext[sig_end..].iter().any(|byte| *byte != 0) {
        return Err(Error::Malformed);
    }
    let body = &plaintext[58..body_end];
    let signature = schnorr::Signature::from_slice(&plaintext[body_end..sig_end]).map_err(|_| Error::Unauthenticated)?;
    let key = XOnlyPublicKey::from_slice(&sender.0).map_err(|_| Error::Unauthenticated)?;
    Secp256k1::verification_only()
        .verify_schnorr(&signature, &digest(kind, recipient, &sender, &id, body), &key)
        .map_err(|_| Error::Unauthenticated)?;
    Ok(Opened {
        sender,
        message_id: id,
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_envelope_round_trips_and_rejects_tampering() {
        let from = SenderKey::generate();
        let recipient = [5u8; 32];
        let plain = encode(Kind::Order, &recipient, &from, b"hello", 512).unwrap();
        assert_eq!(plain.len(), 512);
        let opened = decode(Kind::Order, &recipient, &plain).unwrap();
        assert_eq!((opened.sender, opened.body.as_slice()), (from.id(), &b"hello"[..]));

        assert!(matches!(decode(Kind::Order, &[6u8; 32], &plain), Err(Error::Unauthenticated)), "bound to the recipient");
        assert!(matches!(decode(Kind::Message, &recipient, &plain), Err(Error::Malformed)));
        for at in [6, 40, 58, 58 + 5] {
            let mut bad = plain.clone();
            bad[at] ^= 1;
            assert!(matches!(decode(Kind::Order, &recipient, &bad), Err(Error::Unauthenticated)), "byte {at}");
        }
        let mut padded = plain.clone();
        *padded.last_mut().unwrap() = 1;
        assert!(matches!(decode(Kind::Order, &recipient, &padded), Err(Error::Malformed)));
        assert!(encode(Kind::Order, &recipient, &from, &[0; 400], 512).is_err());
    }

    #[test]
    fn derived_keys_repeat_per_conversation_and_differ_across_them() {
        let root = [7u8; 32];
        let a = SenderKey::derive(&root, "order", b"one");
        assert_eq!(a.id(), SenderKey::derive(&root, "order", b"one").id());
        assert_ne!(a.id(), SenderKey::derive(&root, "order", b"two").id());
        assert_ne!(a.id(), SenderKey::derive(&root, "escrow", b"one").id());
        assert_ne!(a.id(), SenderKey::derive(&[8u8; 32], "order", b"one").id());
    }
}
