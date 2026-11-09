//! Tor order mailbox for OpenBazaar.
//!
//! A listing is public and replicated. An order is a private letter to one
//! vendor. [`seal_order`] signs it with a per-order [`SenderKey`], pads it
//! to a size bucket, and encrypts it to that vendor's mailbox key. An
//! [`OrderTransport`] carries the ciphertext to the vendor's v3 onion
//! address. No public relay stores "this key wrote to that key."
//!
//! The home node is the mailbox. [`start_onion_listener`] is the library
//! call the node binary uses to accept letters while the vendor is away.
//! It is not a daemon. The desktop app later [`Mailbox::take`]s a letter
//! and [`open_order`]s it with the store secret. The ship-to address is a
//! field inside that plaintext. The seller is supposed to learn it.
//!
//! The phone is the buyer. A vendor with no awake node is closed: nothing
//! in this crate stores the letter on a third-party relay.
//!
//! # What talks to Arti
//!
//! * [`OnionAddress::parse`] validates a v3 address with Arti's `HsId`.
//! * [`ArtiOrderTransport`] dials that address with `TorClient::connect`.
//! * [`start_onion_listener`] calls `TorClient::launch_onion_service`.
//! * [`start_onion_forward`] runs an onion service that pipes one port to a
//!   local TCP address (the node's remote admin gateway), and [`exchange`]
//!   sends one request to such a service.
//!
//! [`MemoryTransport`] is the in-memory fake. Default tests use it and never
//! open a Tor circuit. The live dial is an ignored test.

#![forbid(unsafe_code)]

mod client;
mod error;
mod forward;
mod guard;
pub mod letter;
mod listen;
mod mailbox;
mod onion;
mod order;
pub mod reply;
mod seal;
mod transport;

pub use error::Error;
pub use forward::{discovery_public, exchange, insert_discovery_key, onion_for_seed, start_onion_forward, Discovery, OnionForward};
pub use listen::{is_shop_preface, start_onion_listener, Arrival, OnionListener, RunningMailbox};
pub use guard::{ReplayGuard, SenderPins};
pub use letter::{MessageId, Opened, SenderId, SenderKey};
pub use mailbox::{LetterId, Mailbox};
pub use onion::{HsIdParseError, OnionAddress, ORDER_PORT};
pub use order::{ListingHash, Order, OrderTerms, PaymentRail, TermsLine, NO_SERVICE};
pub use reply::{reply_frame, DropId, ReplyDelivery, ReplyPath};
pub use seal::{
    open_message, open_order, open_reply, seal_message, seal_order, seal_reply, ReplyKeypair,
    ReplyPublic, ReplySecret, SealedLetter, SealedReply, StoreKey, StoreKeypair, StoreSecret,
    LETTER_BUCKETS, MAX_MESSAGE_LEN, MAX_REPLY_LEN, REPLY_BUCKETS,
};
pub use transport::{ArtiOrderTransport, MemoryTransport, OrderTransport};

pub use tor_hscrypto::pk::HsId;
