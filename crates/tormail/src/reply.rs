//! How a vendor's reply gets back to a buyer who is never located.
//!
//! The sealed order carries a [`ReplyPath`]: a fresh X25519 reply key, the
//! buyer's reply mailbox onion (when the buyer's node runs one), and a random
//! drop id. All three are per order and travel only inside the ciphertext, so
//! the vendor learns no IP address and nothing that links this order to the
//! buyer's other orders, and no relay learns that this buyer wrote to this
//! vendor. A buyer node should give each order its own reply onion; one onion
//! shared across orders would let vendors who compare notes link them.
//!
//! The vendor seals a reply to the reply key ([`crate::seal_reply`]) and hands
//! a [`ReplyDelivery`] to the transport, which dials the onion and writes one
//! [`ReplyDelivery::to_frame`] frame. The buyer's listener files it by drop id
//! and opens it with the per-order reply secret. Until the transport exists,
//! `onion` is `None` and [`ReplyPath::delivery`] returns `None`: the reply is
//! sealed but has nowhere to go.
//!
//! ```text
//! OBRF | 0x01 | drop id (16) | reply length (u32) | OBSR reply
//! ```

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::onion::OnionAddress;
use crate::seal::{ReplyPublic, SealedReply};
use crate::Error;

const FRAME_MAGIC: &[u8; 4] = b"OBRF";
const FRAME_VERSION: u8 = 1;

/// Routes a reply to one order at the buyer's mailbox. Random per order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DropId(pub [u8; 16]);

impl DropId {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 16];
        StdRng::from_os_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }
}

/// Where and to which key a vendor answers one order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyPath {
    /// Per-order X25519 key. The buyer keeps the secret.
    pub key: ReplyPublic,
    /// Per-order reply mailbox onion. `None` until the buyer node runs one.
    pub onion: Option<OnionAddress>,
    pub drop: DropId,
}

impl ReplyPath {
    /// A path with no onion yet: the reply can be sealed, not delivered.
    pub fn sealed_only(key: ReplyPublic, drop: DropId) -> Self {
        Self { key, onion: None, drop }
    }

    /// What the transport carries, when the buyer gave an onion.
    pub fn delivery(&self, reply: SealedReply) -> Option<ReplyDelivery> {
        Some(ReplyDelivery {
            onion: self.onion.clone()?,
            drop: self.drop,
            reply,
        })
    }
}

/// One sealed reply on its way to a buyer's reply onion.
#[derive(Clone, Debug)]
pub struct ReplyDelivery {
    pub onion: OnionAddress,
    pub drop: DropId,
    pub reply: SealedReply,
}

impl ReplyDelivery {
    /// The bytes written on the reply stream. The onion is the dial target, not part of the frame.
    pub fn to_frame(&self) -> Vec<u8> {
        let reply = self.reply.to_bytes();
        let mut out = Vec::with_capacity(4 + 1 + 16 + 4 + reply.len());
        out.extend_from_slice(FRAME_MAGIC);
        out.push(FRAME_VERSION);
        out.extend_from_slice(&self.drop.0);
        out.extend_from_slice(&(reply.len() as u32).to_le_bytes());
        out.extend_from_slice(&reply);
        out
    }

    /// Read a frame on the buyer's side: the drop id and the sealed reply.
    pub fn from_frame(bytes: &[u8]) -> Result<(DropId, SealedReply), Error> {
        if bytes.len() < 25 || &bytes[..4] != FRAME_MAGIC || bytes[4] != FRAME_VERSION {
            return Err(Error::Malformed);
        }
        let drop = DropId(bytes[5..21].try_into().expect("16 bytes"));
        let len = u32::from_le_bytes(bytes[21..25].try_into().expect("4 bytes")) as usize;
        if bytes.len() - 25 != len {
            return Err(Error::Malformed);
        }
        Ok((drop, SealedReply::from_bytes(&bytes[25..])?))
    }
}
