//! Tor order mailbox for OpenBazaar.
//!
//! A listing is public and replicated. An order is a private letter to one
//! vendor. [`seal_order`] encrypts it to that vendor's store key. An
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
//!
//! [`MemoryTransport`] is the in-memory fake. Default tests use it and never
//! open a Tor circuit. The live dial is an ignored test.

#![forbid(unsafe_code)]

mod error;
mod listen;
mod mailbox;
mod onion;
mod order;
mod seal;
mod transport;

pub use error::Error;
pub use listen::{start_onion_listener, OnionListener};
pub use mailbox::{LetterId, Mailbox};
pub use onion::{HsIdParseError, OnionAddress, ORDER_PORT};
pub use order::{ListingHash, Order, PaymentRail};
pub use seal::{
    open_message, open_order, open_reply, seal_message, seal_order, seal_reply, ReplyKeypair,
    ReplyPublic, ReplySecret, SealedLetter, SealedReply, StoreKey, StoreKeypair, StoreSecret,
    MAX_MESSAGE_LEN,
};
pub use transport::{ArtiOrderTransport, MemoryTransport, OrderTransport};

pub use tor_hscrypto::pk::HsId;
