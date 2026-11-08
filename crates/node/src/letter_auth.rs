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
//! The reply onion is a different derivation (`reply-onion` or
//! `escrow-reply-onion`). It is not the store's `ob-sale` onion, so a seller
//! or moderator who sees two reply addresses cannot tell they are one node.
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
        Kind::Offer | Kind::Reply | Kind::ReleaseNonce | Kind::VendorDispute => 2,
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

    /// Refuse a replayed letter or one from a key this escrow is not pinned
    /// to, without recording it: a letter whose step is deferred or fails
    /// is read again.
    pub(crate) fn check(&self, opened: &Opened<Vec<u8>>, escrow: &EscrowId, kind: Kind) -> Result<(), String> {
        self.pins
            .check(escrow, sender_role(kind), &opened.sender)
            .map_err(|err| err.to_string())?;
        if self.seen.contains(&opened.message_id).map_err(|err| err.to_string())? {
            return Err(tormail::Error::Replay.to_string());
        }
        Ok(())
    }

    /// Record the letter's message id, refusing a replay.
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

    /// The buyer's per-order review key: a receipt names it, a review is
    /// signed with it, and it links to nothing else the buyer does.
    pub(crate) fn order_review_secret(&self, checkout_id: &str, store: &[u8; 32]) -> Result<bitcoin::secp256k1::SecretKey, String> {
        let root = self.letter_root()?;
        let conversation = Self::order_conversation(checkout_id, store);
        (0..8)
            .find_map(|n| bitcoin::secp256k1::SecretKey::from_slice(&derive_secret(&root, "review-key", &conversation, n)).ok())
            .ok_or_else(|| "no review key".to_string())
    }

    /// Seed for this order's reply onion. The same inputs always yield the
    /// same onion, and a different checkout or store yields a different one.
    pub(crate) fn order_reply_onion_seed(&self, checkout_id: &str, store: &[u8; 32]) -> Result<[u8; 32], String> {
        Ok(reply_onion_seed(&*self.letter_root()?, "reply-onion", &Self::order_conversation(checkout_id, store)))
    }

    /// Seed for this escrow's reply onion. It is not the store onion.
    pub(crate) fn escrow_reply_onion_seed(&self, escrow: &EscrowId) -> Result<[u8; 32], String> {
        Ok(reply_onion_seed(&*self.letter_root()?, "escrow-reply-onion", escrow))
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

    /// Republish reply onions for orders and escrows already on this node.
    /// Called once the sale gate is up, so a restart still receives replies.
    pub(crate) fn republish_reply_onions(&self) {
        if self.remote.services.sale_onion().is_none() {
            return;
        }
        let open: Vec<(String, Option<[u8; 32]>, Option<crate::escrow_mail::EscrowId>)> = self
            .orders()
            .on_network()
            .map(|order| {
                let store = order.remote.is_some().then(|| crate::stores::unhex32(&order.store_id)).flatten();
                let escrow = order.escrow.as_ref().and_then(crate::escrow_buyer::escrow_id_of);
                (order.checkout_id.clone(), store, escrow)
            })
            .collect();
        let mut seeds = Vec::new();
        for (checkout_id, store, escrow) in open {
            if let Some(store) = store {
                if let Ok(seed) = self.order_reply_onion_seed(&checkout_id, &store) {
                    seeds.push(seed);
                }
            }
            if let Some(id) = escrow {
                if let Ok(seed) = self.escrow_reply_onion_seed(&id) {
                    seeds.push(seed);
                }
            }
        }
        for id in self.vendor_escrow_ids() {
            if let Ok(seed) = self.escrow_reply_onion_seed(&id) {
                seeds.push(seed);
            }
        }
        for seed in seeds {
            if let Err(err) = self.remote.publish_reply_onion(&self.data_dir, &seed) {
                eprintln!("ob-node: reply onion: {err}");
            }
        }
    }
}

/// 32 bytes that [`tormail::onion_for_seed`] turns into this conversation's onion.
pub(crate) fn reply_onion_seed(root: &[u8; 32], purpose: &str, conversation: &[u8]) -> [u8; 32] {
    derive_secret(root, purpose, conversation, 0)
}

/// `http://….onion` for `seed`. No port: the virtual port is 80.
pub(crate) fn reply_onion_url(seed: &[u8; 32]) -> String {
    format!("http://{}", tormail::onion_for_seed(seed))
}

/// The address a remote peer is told. A per-order onion wins over the node's
/// mailbox, which is the store onion once `--remote` has published it.
pub(crate) fn choose_reply(per_order: Option<&str>, mailbox: Option<&str>) -> Option<String> {
    per_order.or(mailbox).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{choose_reply, reply_onion_seed, reply_onion_url};

    #[test]
    fn two_orders_do_not_share_a_reply_onion() {
        let root = [4u8; 32];
        let store = [8u8; 32];
        let store_onion = "http://store.example.onion";
        let a = reply_onion_url(&reply_onion_seed(&root, "reply-onion", b"checkout-a"));
        let b = reply_onion_url(&reply_onion_seed(&root, "reply-onion", b"checkout-b"));
        let again = reply_onion_url(&reply_onion_seed(&root, "reply-onion", b"checkout-a"));
        let other_store = reply_onion_url(&reply_onion_seed(&root, "reply-onion", &[&b"checkout-a"[..], &store].concat()));
        assert_ne!(a, b, "two checkouts shared a reply onion");
        assert_eq!(a, again, "the same order must keep its onion across a restart");
        assert_ne!(a, other_store, "two stores in one checkout shared a reply onion");
        assert!(a.strip_prefix("http://").is_some_and(|host| host.ends_with(".onion") && !host.contains(':')), "{a}");
        assert_eq!(choose_reply(Some(&a), Some(store_onion)).as_deref(), Some(a.as_str()));
        assert_ne!(choose_reply(Some(&a), Some(store_onion)).as_deref(), Some(store_onion));
        assert_eq!(choose_reply(None, Some("http://127.0.0.1:9")).as_deref(), Some("http://127.0.0.1:9"));
    }
}
