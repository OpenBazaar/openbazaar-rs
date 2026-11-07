//! Which key signs each letter this node sends, and what it checks on letters it receives.
//!
//! Every letter is signed inside its ciphertext by a key for one
//! conversation (see `tormail::letter` for why). The node derives that key
//! from the letter root in its backup phrase and the conversation id, so it
//! stores nothing per conversation and two conversations get unrelated keys:
//!
//! | letter            | conversation id                     |
//! |-------------------|-------------------------------------|
//! | order             | `order` ‖ checkout id ‖ store key   |
//! | escrow mail       | `escrow` ‖ sender role ‖ escrow id  |
//!
//! On receipt, escrow mail must carry a message id this node has not seen
//! (`letters-seen`) and, after the first letter of an escrow from a given
//! peer role was handled, the same sender key as that letter
//! (`letter-pins`). A letter that fails either check is dropped before any
//! escrow code reads it.

use std::path::Path;

use tormail::letter::derive_secret;
use tormail::{Opened, ReplayGuard, ReplySecret, SenderKey, SenderPins};
use zeroize::Zeroizing;

use crate::escrow_mail::{EscrowId, Kind};
use crate::{Error, Node};

/// Who wrote an escrow letter, by its kind.
pub(crate) fn sender_role(kind: Kind) -> u8 {
    match kind {
        Kind::Request | Kind::Open | Kind::Signatures | Kind::Release | Kind::Dispute => 1,
        Kind::Offer | Kind::Reply | Kind::ReleaseNonce => 2,
        Kind::Ruling => 3,
    }
}

pub(crate) struct LetterGuard {
    seen: ReplayGuard,
    pins: SenderPins,
}

impl LetterGuard {
    pub(crate) fn open(dir: &Path) -> Result<Self, Error> {
        let detail = |err: tormail::Error| Error::Detail(err.to_string());
        Ok(Self {
            seen: ReplayGuard::open(dir.join("letters-seen")).map_err(detail)?,
            pins: SenderPins::open(dir.join("letter-pins")).map_err(detail)?,
        })
    }

    /// Refuse a replayed letter or one from a key this escrow is not pinned to.
    pub(crate) fn admit(&self, opened: &Opened<Vec<u8>>, escrow: &EscrowId, kind: Kind) -> Result<(), String> {
        self.pins
            .check(escrow, sender_role(kind), &opened.sender)
            .map_err(|err| err.to_string())?;
        self.seen.admit(&opened.message_id).map_err(|err| err.to_string())
    }

    /// Bind the escrow to this letter's sender once the letter was handled.
    pub(crate) fn pin(&self, opened: &Opened<Vec<u8>>, escrow: &EscrowId, kind: Kind) -> Result<(), String> {
        self.pins
            .pin(escrow, sender_role(kind), &opened.sender)
            .map_err(|err| err.to_string())
    }
}

impl Node {
    /// The letter root, from the receive tier: a locked node still signs
    /// letters. It never signs a transaction, an escrow step, or a store record.
    fn letter_root(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        self.buyer_wallet().watch.letter_root().map(|root| Zeroizing::new(*root))
    }

    /// The key this node signs `kind` escrow mail with in escrow `escrow`.
    pub(crate) fn escrow_letter_key(&self, kind: Kind, escrow: &EscrowId) -> Result<SenderKey, String> {
        let conversation = [&[sender_role(kind)][..], escrow].concat();
        Ok(SenderKey::derive(&*self.letter_root()?, "escrow", &conversation))
    }

    fn order_conversation(checkout_id: &str, store: &[u8; 32]) -> Vec<u8> {
        [checkout_id.as_bytes(), store].concat()
    }

    /// The buyer's signing key for one order letter.
    pub(crate) fn order_letter_key(&self, checkout_id: &str, store: &[u8; 32]) -> Result<SenderKey, String> {
        Ok(SenderKey::derive(&*self.letter_root()?, "order", &Self::order_conversation(checkout_id, store)))
    }

    /// The buyer's per-order reply secret and drop id. Derived again from
    /// the order record when a reply arrives; never stored.
    pub(crate) fn order_reply_secret(&self, checkout_id: &str, store: &[u8; 32]) -> Result<(ReplySecret, tormail::DropId), String> {
        let conversation = Self::order_conversation(checkout_id, store);
        let root = self.letter_root()?;
        let secret = derive_secret(&root, "reply-key", &conversation, 0);
        let drop = derive_secret(&root, "reply-drop", &conversation, 0);
        Ok((
            ReplySecret::from_bytes(secret).expect("any 32 bytes are an X25519 secret"),
            tormail::DropId(drop[..16].try_into().expect("16 bytes")),
        ))
    }
}
