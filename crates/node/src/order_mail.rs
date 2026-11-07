//! Order mail between a buyer's node and a seller's node.
//!
//! Three kinds of bytes cross, all over loopback HTTP for now (the same
//! transport as escrow mail), all sealed with tormail:
//!
//! | from   | to                      | what                                           |
//! |--------|-------------------------|------------------------------------------------|
//! | buyer  | `POST {store}/mail`     | the order letter, sealed to the store's mailbox key |
//! | buyer  | `POST {store}/mail`     | a follow-up message in that order ("complete") |
//! | seller | `POST {buyer}/mail/reply` | a reply frame, sealed to the order's reply key |
//!
//! The buyer's base URL travels inside the sealed order as the `replyUrl`
//! option, next to the per-order reply key and drop id. The store's URL is
//! the one the buyer follows ([`crate::replicas`]).
//!
//! Everything sent goes through a persistent outbox (`order-outbox/`), so a
//! seller that is offline gets the order when it is back, and a buyer that
//! is offline gets its replies. Replies are only queued, never sent while a
//! request is being answered: the buyer's node may be waiting on that very
//! request. The order loop flushes the outbox every second.
//!
//! Every step a letter can trigger is idempotent, so a letter delivered twice
//! (a lost response, then a retry) changes nothing the second time. After the
//! first letter of an order each side pins the other's per-order sender key.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash, HashEngine};
use serde_json::Value;
use tormail::SenderKey;

use crate::escrow_outbox::{now_secs, Outbox, UrlTransport};
use crate::vault::Vault;
use crate::{Error, Node};

const KEY_TAG: &[u8] = b"ob-node/order-mail/v1";
pub(crate) const STORE_MAIL: &str = "/mail";
pub(crate) const REPLY_MAIL: &str = "/mail/reply";

/// Why a peer's mail was not taken. A refusal is a 4xx (the sender drops
/// it); a retry is a 503 (the sender's outbox tries again).
#[derive(Debug)]
pub(crate) enum MailError {
    Refused(String),
    Retry(String),
}

pub(crate) fn open_outbox(dir: &std::path::Path, vault: Vault) -> Result<Outbox, Error> {
    Outbox::open_named(dir, "order-outbox", vault, Box::new(UrlTransport))
}

/// The outbox key of `body` to `url` in conversation `conversation`: the
/// same message queued twice is one entry.
pub(crate) fn mail_key(url: &str, conversation: &[u8], body: &[u8]) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(KEY_TAG);
    for part in [url.as_bytes(), conversation, body] {
        engine.input(&(part.len() as u32).to_le_bytes());
        engine.input(part);
    }
    sha256::Hash::from_engine(engine).to_byte_array()
}

impl Node {
    /// The seller's signing key for replies in one sale, from the receive tier.
    pub(crate) fn sale_letter_key(&self, drop: &[u8; 16]) -> Result<SenderKey, String> {
        let wallet = self.buyer_wallet();
        Ok(SenderKey::derive(wallet.watch.letter_root()?, "sale", drop))
    }

    /// Send every order letter and reply that is due.
    pub(crate) fn flush_order_mail(&self, force: bool) -> usize {
        self.order_outbox.flush(now_secs(), force)
    }

    pub(crate) fn run_order_mail(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            self.flush_order_mail(false);
            std::thread::sleep(Duration::from_millis(1000));
        }
    }
}

/// A JSON body for a sealed letter or reply.
pub(crate) fn body_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("mail json is finite")
}
