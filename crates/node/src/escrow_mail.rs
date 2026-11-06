//! Escrow messages between nodes.
//!
//! Every escrow round is a tormail [`SealedLetter`] sealed to the recipient's
//! mailbox key, POSTed to the recipient node's `/escrow/mail` on loopback
//! HTTP, stored in its `escrow-mail/` mailbox, and opened by
//! [`Node::process_escrow_mail`]. Which of the node's keys opens a letter
//! decides the role it is handled in: the node's own mail key (buyer), a
//! seller's order mailbox key (vendor), or the fixture moderator's key.
//!
//! A node never delivers to its own URL: one process would then hold both
//! per-order keys, which is exactly what the two-party protocol rules out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use bitcoin::hex::{DisplayHex, FromHex};
use tormail::{open_message, seal_message, Mailbox, SealedLetter, StoreKey, StoreSecret};

use crate::escrow_buyer::BuyerDesk;
use crate::escrow_fixture::{moderator_mail_secret, vendor_mail_secret};
use crate::escrow_moderator::ModeratorDesk;
use crate::escrow_vendor::VendorDesk;
use crate::network::NetworkMode;
use crate::{EscrowConfig, Node};

const MAGIC: &[u8; 4] = b"OBEM";
const VERSION: u8 = 1;
const MAX_FIELDS: usize = 8;
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAIL_POLL: Duration = Duration::from_millis(250);

pub(crate) type EscrowId = [u8; 16];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Buyer to vendor: buyer key, payout, mail key, reply URL, funding sats.
    Request = 1,
    /// Vendor to buyer: terms, vendor terms signature, store signature.
    Offer = 2,
    /// Buyer to vendor: signed terms, `BuyerOpen`.
    Open = 3,
    /// Vendor to buyer: `VendorReply`.
    Reply = 4,
    /// Buyer to vendor: `BuyerSignatures`.
    Signatures = 5,
    /// Vendor to buyer: the vendor's public release nonce.
    ReleaseNonce = 6,
    /// Buyer to vendor: `ReleaseSignature`.
    Release = 7,
    /// Buyer to moderator: dispute letter, mail key, reply URL.
    Dispute = 8,
    /// Moderator to buyer: `ModeratorRuling`.
    Ruling = 9,
}

impl Kind {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Kind::Request,
            2 => Kind::Offer,
            3 => Kind::Open,
            4 => Kind::Reply,
            5 => Kind::Signatures,
            6 => Kind::ReleaseNonce,
            7 => Kind::Release,
            8 => Kind::Dispute,
            9 => Kind::Ruling,
            _ => return None,
        })
    }
}

/// One escrow message: a kind, the escrow it belongs to, and opaque fields.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Mail {
    pub kind: Kind,
    pub escrow: EscrowId,
    fields: Vec<Vec<u8>>,
}

impl Mail {
    pub fn new(kind: Kind, escrow: EscrowId, fields: Vec<Vec<u8>>) -> Self {
        Self {
            kind,
            escrow,
            fields,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(self.kind as u8);
        out.extend_from_slice(&self.escrow);
        out.push(self.fields.len() as u8);
        for field in &self.fields {
            out.extend_from_slice(&(field.len() as u32).to_le_bytes());
            out.extend_from_slice(field);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let bad = || "the escrow message could not be read".to_string();
        let header = 4 + 1 + 1 + 16 + 1;
        if bytes.len() < header || &bytes[..4] != MAGIC || bytes[4] != VERSION {
            return Err(bad());
        }
        let kind = Kind::from_u8(bytes[5]).ok_or_else(bad)?;
        let escrow: EscrowId = bytes[6..22].try_into().map_err(|_| bad())?;
        let count = bytes[22] as usize;
        if count > MAX_FIELDS {
            return Err(bad());
        }
        let mut rest = &bytes[header..];
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            if rest.len() < 4 {
                return Err(bad());
            }
            let len = u32::from_le_bytes(rest[..4].try_into().map_err(|_| bad())?) as usize;
            rest = &rest[4..];
            if rest.len() < len {
                return Err(bad());
            }
            fields.push(rest[..len].to_vec());
            rest = &rest[len..];
        }
        if !rest.is_empty() {
            return Err(bad());
        }
        Ok(Self {
            kind,
            escrow,
            fields,
        })
    }

    /// The fields, refusing a message that does not carry exactly `count`.
    pub fn fields(&self, count: usize) -> Result<&[Vec<u8>], String> {
        if self.fields.len() != count {
            return Err(format!("a {:?} message carries {count} fields", self.kind));
        }
        Ok(&self.fields)
    }
}

pub(crate) fn id_hex(id: &EscrowId) -> String {
    id.to_lower_hex_string()
}

pub(crate) fn parse_id(text: &str) -> Option<EscrowId> {
    <[u8; 16]>::from_hex(text).ok()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.to_lower_hex_string()
}

pub(crate) fn unhex(text: &str) -> Result<Vec<u8>, String> {
    Vec::<u8>::from_hex(text).map_err(|_| "a stored escrow field is not hex".to_string())
}

pub(crate) fn key_bytes(bytes: &[u8]) -> Result<StoreKey, String> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "a mail key is 32 bytes".to_string())?;
    StoreKey::from_bytes(bytes).map_err(|err| err.to_string())
}

/// Refuse anything but a plain-HTTP loopback base URL.
pub(crate) fn check_loopback(url: &str) -> Result<(), String> {
    let refused = || format!("{url} is not a loopback http:// URL");
    let rest = url.strip_prefix("http://").ok_or_else(refused)?;
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    if host == "127.0.0.1" || host == "localhost" || host == "::1" {
        Ok(())
    } else {
        Err(refused())
    }
}

fn deliver(base_url: &str, letter: &SealedLetter) -> Result<(), String> {
    check_loopback(base_url)?;
    let url = format!("{}/escrow/mail", base_url.trim_end_matches('/'));
    ureq::post(&url)
        .timeout(DELIVERY_TIMEOUT)
        .set("Content-Type", "application/octet-stream")
        .send_bytes(&letter.to_bytes())
        .map_err(|err| format!("{url}: {err}"))?;
    Ok(())
}

/// This node's escrow inbox and the state each role keeps between rounds.
pub(crate) struct Escrow {
    pub config: EscrowConfig,
    pub inbox: Mailbox,
    pub buyer: Mutex<BuyerDesk>,
    pub vendor: Mutex<VendorDesk>,
    pub moderator: Mutex<ModeratorDesk>,
}

impl Escrow {
    pub fn new(config: EscrowConfig, inbox: Mailbox) -> Self {
        Self {
            config,
            inbox,
            buyer: Mutex::new(BuyerDesk::default()),
            vendor: Mutex::new(VendorDesk::default()),
            moderator: Mutex::new(ModeratorDesk::default()),
        }
    }
}

impl Node {
    /// Seal `mail` to `to` and post it to the node at `url`.
    pub(crate) fn send_mail(&self, url: &str, to: &StoreKey, mail: &Mail) -> Result<(), String> {
        if self.own_url() == Some(url.trim_end_matches('/')) {
            return Err("an escrow peer must be another node, not this one".into());
        }
        let letter = seal_message(&mail.encode(), to).map_err(|err| err.to_string())?;
        deliver(url, &letter)
    }

    pub(crate) fn own_url(&self) -> Option<&str> {
        self.escrow
            .config
            .mailbox_url
            .as_deref()
            .map(|url| url.trim_end_matches('/'))
    }

    /// The URL peers reply to. Escrow needs one.
    pub(crate) fn reply_url(&self) -> Result<&str, String> {
        self.own_url().ok_or_else(|| {
            "This node has no escrow mailbox URL, so peers cannot reply.".to_string()
        })
    }

    /// Opens escrow mail sent to this node as buyer.
    pub(crate) fn buyer_mail_secret(&self) -> StoreSecret {
        let secret = self.buyer_wallet().seed.mail_key().secret_bytes();
        StoreSecret::from_bytes(secret).expect("any 32 bytes are an X25519 secret")
    }

    /// Accept one sealed letter from a peer into the escrow inbox.
    pub(crate) fn accept_escrow_mail(&self, bytes: &[u8]) -> Result<String, String> {
        if self.network != NetworkMode::Testnet4 {
            return Err(crate::instructions::ESCROW_MAINNET_REFUSAL.into());
        }
        let letter = SealedLetter::from_bytes(bytes).map_err(|err| err.to_string())?;
        let id = self
            .escrow
            .inbox
            .accept(&letter)
            .map_err(|err| err.to_string())?;
        Ok(id.as_str())
    }

    /// Open and handle every letter waiting in the escrow inbox. Returns how
    /// many were handled; a letter that fails is logged and dropped.
    pub fn process_escrow_mail(&self) -> usize {
        if self.network != NetworkMode::Testnet4 {
            return 0;
        }
        let ids = match self.escrow.inbox.list() {
            Ok(ids) => ids,
            Err(err) => {
                eprintln!("ob-node: escrow inbox: {err}");
                return 0;
            }
        };
        let mut handled = 0;
        for id in ids {
            let result = self
                .escrow
                .inbox
                .take(&id)
                .map_err(|err| err.to_string())
                .and_then(|letter| self.handle_letter(&letter));
            match result {
                Ok(()) => handled += 1,
                Err(err) => eprintln!("ob-node: escrow letter {}: {err}", id.as_str()),
            }
        }
        handled
    }

    /// Handle escrow mail as it arrives until `stop` is set.
    pub(crate) fn run_escrow_mail(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            self.process_escrow_mail();
            std::thread::sleep(MAIL_POLL);
        }
    }

    fn handle_letter(&self, letter: &SealedLetter) -> Result<(), String> {
        if let Ok(body) = open_message(letter, &self.buyer_mail_secret()) {
            return self.buyer_mail(Mail::decode(&body)?);
        }
        for seller in self.store_wallets.keys() {
            let Some(secret) = vendor_mail_secret(seller) else {
                continue;
            };
            if let Ok(body) = open_message(letter, &secret) {
                return self.vendor_mail(seller, Mail::decode(&body)?);
            }
        }
        if self.escrow.config.moderates {
            if let Ok(body) = open_message(letter, &moderator_mail_secret()) {
                return self.moderator_mail(Mail::decode(&body)?);
            }
        }
        Err("no key on this node opens it".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_round_trips_and_rejects_trailing_or_short_input() {
        let mail = Mail::new(
            Kind::Open,
            [7; 16],
            vec![vec![1, 2, 3], Vec::new(), vec![9; 300]],
        );
        let bytes = mail.encode();
        assert_eq!(Mail::decode(&bytes).unwrap(), mail);
        let mut long = bytes.clone();
        long.push(0);
        assert!(Mail::decode(&long).is_err());
        assert!(Mail::decode(&bytes[..bytes.len() - 1]).is_err());
        let mut kind = bytes.clone();
        kind[5] = 0;
        assert!(Mail::decode(&kind).is_err());
        assert!(mail.fields(2).is_err());
        assert_eq!(mail.fields(3).unwrap().len(), 3);
    }

    #[test]
    fn only_loopback_http_urls_are_peers() {
        for url in [
            "http://127.0.0.1:9101",
            "http://localhost:1/",
            "http://[::1]:80",
        ] {
            assert!(check_loopback(url).is_ok(), "{url}");
        }
        for url in [
            "https://127.0.0.1:1",
            "http://10.0.0.2:1",
            "http://127.0.0.1.evil.com",
            "127.0.0.1:1",
        ] {
            assert!(check_loopback(url).is_err(), "{url}");
        }
    }
}
