//! Escrow messages between nodes.
//!
//! Every escrow round is a tormail [`SealedLetter`] sealed to the recipient's
//! mailbox key, POSTed to the recipient node's `/escrow/mail` on loopback
//! HTTP or that node's sale onion, stored in its `escrow-mail/` mailbox, and opened by
//! [`Node::process_escrow_mail`]. Which of the node's keys opens a letter
//! decides the role it is handled in: the node's own mail key (buyer), a
//! demo store's or the own store's mailbox key (vendor), or the fixture
//! moderator's key.
//!
//! Each letter is signed inside its ciphertext by the sender's key for that
//! escrow and role ([`crate::letter_auth`]). A letter whose signature fails,
//! whose message id was seen before, or whose sender differs from the one
//! this escrow is pinned to is dropped before any desk reads it.
//!
//! A node never delivers to its own URL: one process would then hold both
//! per-order keys, which is exactly what the two-party protocol rules out.
//!
//! Outgoing letters go through the persistent outbox
//! ([`crate::escrow_outbox`]). Each incoming letter runs under its escrow's
//! lock ([`crate::escrow_store::OrderLocks`]). The replay guard records its
//! message id only once its step succeeded, and the letter leaves the inbox
//! only after that, so a letter that was deferred or cut short by a crash
//! runs again. Every step stores its new state before it sends what that
//! state produced, and answers a repeated message from what it stored.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bitcoin::hex::{DisplayHex, FromHex};
use openbazaar_escrow::LetterCache;
use tormail::{open_message, seal_message, Mailbox, Opened, SealedLetter, StoreKey, StoreSecret};

use crate::coin_locks::CoinLocks;
use crate::escrow_buyer::BuyerDesk;
use crate::escrow_fixture::moderator_mail_secret;
use crate::escrow_limits::DisputeLimits;
use crate::escrow_moderator::ModeratorDesk;
use crate::escrow_outbox::{mail_key, HttpTransport, Outbox, Sent};
use crate::escrow_rulings::FileRulingStore;
use crate::escrow_store::{EscrowStore, OrderLocks};
use crate::escrow_vendor::VendorDesk;
use crate::network::NetworkMode;
use crate::vault::Vault;
use crate::{EscrowConfig, Node};

const MAGIC: &[u8; 4] = b"OBEM";
const VERSION: u8 = 1;
const MAX_FIELDS: usize = 8;
const MAIL_POLL: Duration = Duration::from_millis(250);
/// A step that fails with an error starting with this leaves its letter in
/// the inbox to be tried again (the chain view was not ready).
pub(crate) const RETRY_LATER: &str = "not ready yet: ";

pub(crate) type EscrowId = [u8; 16];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Buyer to vendor: buyer key, payout, mail key, reply URL, funding
    /// sats, presigned network fee sats.
    Request = 1,
    /// Vendor to buyer: terms, vendor terms signature, store signature.
    Offer = 2,
    /// Buyer to vendor: signed terms, `BuyerOpen`, attempt (u32 LE). A
    /// buyer that restarted in round 1 opens again with the next attempt.
    Open = 3,
    /// Vendor to buyer: `VendorReply`, the attempt it answers.
    Reply = 4,
    /// Buyer to vendor: `BuyerSignatures`.
    Signatures = 5,
    /// Vendor to buyer: the vendor's public release nonce.
    ReleaseNonce = 6,
    /// Buyer to vendor: `ReleaseSignature`.
    Release = 7,
    /// Buyer to moderator: dispute letter, mail key, reply URL.
    Dispute = 8,
    /// Moderator to the party that wrote the letter: `ModeratorRuling`.
    Ruling = 9,
    /// Vendor to moderator: dispute letter, mail key, reply URL. Its own
    /// kind so the letter is signed and pinned in the vendor's role.
    VendorDispute = 10,
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
            10 => Kind::VendorDispute,
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

    pub fn as_fields(&self) -> &[Vec<u8>] {
        &self.fields
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

/// One request to another node on this machine.
///
/// Redirects are off. A seller, or anyone answering on the loopback URL, must
/// not bounce the request to a public host: the next connection would leave
/// this machine and show its IP, and would carry the review, the catalog
/// fetch, or the sealed letter with it.
pub(crate) fn loopback_request(method: &str, url: &str, timeout: Duration) -> Result<ureq::Request, String> {
    check_loopback(url)?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .build();
    Ok(agent.request(method, url).timeout(timeout))
}

/// This node's escrow inbox, outbox, durable state, and the state each role
/// keeps in memory between rounds.
pub(crate) struct Escrow {
    pub config: EscrowConfig,
    pub inbox: Mailbox,
    pub outbox: Outbox,
    pub store: EscrowStore,
    pub locks: OrderLocks,
    pub coins: CoinLocks,
    pub rulings: FileRulingStore,
    pub limits: DisputeLimits,
    pub letters: Arc<LetterCache>,
    pub buyer: Mutex<BuyerDesk>,
    pub vendor: Mutex<VendorDesk>,
    pub moderator: Mutex<ModeratorDesk>,
    /// The moderator profiles this node knows and the ones its stores accept.
    pub book: Mutex<crate::moderators::Book>,
    /// This node's own moderator identity, once a `--moderate` node has made one.
    pub identity: Mutex<Option<crate::moderator_identity::Identity>>,
}

impl Escrow {
    pub fn open(
        config: EscrowConfig,
        inbox: Mailbox,
        dir: &Path,
        vault: &Vault,
        peers: Arc<crate::peer_http::PeerLink>,
    ) -> Result<Self, crate::Error> {
        let store = EscrowStore::open(dir, vault.clone())?;
        let vendor = VendorDesk::load(&store);
        let moderator = ModeratorDesk::load(&store);
        Ok(Self {
            config,
            inbox,
            outbox: Outbox::open(dir, vault.clone(), Box::new(HttpTransport::new(peers)))?,
            store,
            locks: OrderLocks::default(),
            coins: CoinLocks::open(dir, vault.clone())?,
            rulings: FileRulingStore::open(dir, vault.clone())?,
            limits: DisputeLimits::default(),
            letters: Arc::default(),
            buyer: Mutex::new(BuyerDesk::default()),
            vendor: Mutex::new(vendor),
            moderator: Mutex::new(moderator),
            book: Mutex::new(crate::moderators::Book::open(dir)?),
            identity: Mutex::new(crate::moderator_identity::Identity::open(dir, vault)?),
        })
    }
}

impl Node {
    /// Seal `mail` to `to` and queue it for the node at `url`, trying it once
    /// now. [`Sent::Queued`] means the outbox keeps trying; an error means
    /// the peer refused it or it could not be queued.
    pub(crate) fn send_mail(&self, url: &str, to: &StoreKey, mail: &Mail) -> Result<Sent, String> {
        let url = crate::peer_http::sale_base(url)?;
        if self.own_url() == Some(url.as_str()) || self.sale_reply_url().as_deref() == Some(url.as_str()) {
            return Err("an escrow peer must be another node, not this one".into());
        }
        let from = self.escrow_letter_key(mail.kind, &mail.escrow)?;
        let letter = seal_message(&mail.encode(), to, &from).map_err(|err| err.to_string())?;
        self.escrow
            .outbox
            .send(mail_key(&url, mail), &url, &letter.to_bytes())
    }

    /// Retry queued escrow letters that are due (all of them with `force`).
    pub(crate) fn flush_escrow_outbox(&self, force: bool) -> usize {
        self.escrow.outbox.flush(crate::escrow_outbox::now_secs(), force)
    }

    pub(crate) fn own_url(&self) -> Option<&str> {
        self.escrow
            .config
            .mailbox_url
            .as_deref()
            .map(|url| url.trim_end_matches('/'))
    }

    /// The URL peers reply to. The sale onion when `--remote` has published
    /// one, otherwise this node's loopback mailbox.
    pub(crate) fn reply_url(&self) -> Result<String, String> {
        self.sale_reply_url().ok_or_else(|| {
            "This node has no escrow mailbox URL, so peers cannot reply.".to_string()
        })
    }

    /// Where this escrow's peer sends the next letter. With the sale onion
    /// up, this is an onion for `id` alone, forwarded to the sale gate.
    /// Loopback demos keep [`Self::reply_url`].
    pub(crate) fn escrow_reply_url(&self, id: &EscrowId) -> Result<String, String> {
        if self.remote.services.sale_onion().is_none() {
            return self.reply_url();
        }
        let seed = self.escrow_reply_onion_seed(id)?;
        self.remote.publish_reply_onion(&self.data_dir, &seed)
    }

    /// Opens escrow mail sent to this node as buyer.
    pub(crate) fn buyer_mail_secret(&self) -> StoreSecret {
        let secret = *self.buyer_wallet().watch.mail_key;
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
    /// many were handled; a letter that fails is logged and dropped. A letter
    /// whose step needs the spend tier stays in the inbox while the wallet is
    /// locked and is handled after the next unlock.
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
                .read(&id)
                .map_err(|err| err.to_string())
                .and_then(|letter| self.handle_letter(&letter));
            if matches!(result, Ok(Handled::Deferred)) {
                continue;
            }
            if let Err(err) = self.escrow.inbox.take(&id) {
                eprintln!("ob-node: escrow letter {}: {err}", id.as_str());
            }
            match result {
                Ok(_) => handled += 1,
                Err(err) => eprintln!("ob-node: escrow letter {}: {err}", id.as_str()),
            }
        }
        handled
    }

    /// Handle escrow mail as it arrives, and retry the outbox, until `stop` is set.
    pub(crate) fn run_escrow_mail(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            self.process_escrow_mail();
            self.flush_escrow_outbox(false);
            self.refresh_moderator_identity();
            std::thread::sleep(MAIL_POLL);
        }
    }

    /// Open a letter with the receive tier, then handle it, or defer it
    /// when its step signs with a spend-tier key and the wallet is locked.
    /// The replay guard is only checked before the step and records the
    /// letter after it, so a deferred letter is read again later.
    pub(crate) fn handle_letter(&self, letter: &SealedLetter) -> Result<Handled, String> {
        let unlocked = self.lock.state() == crate::wallet_lock::State::Unlocked;
        let (desk, opened) = self.open_letter(letter)?;
        let mail = Mail::decode(&opened.body)?;
        let signs = match desk {
            Desk::Buyer => buyer_step_signs(mail.kind),
            // Every vendor round uses the sale's per-order key.
            Desk::Vendor(_) => true,
            Desk::Moderator => false,
        };
        if signs && !unlocked {
            return Ok(Handled::Deferred);
        }
        let (escrow, kind) = (mail.escrow, mail.kind);
        let lock = self.escrow.locks.get(&escrow);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        self.letter_guard.check(&opened, &escrow, kind)?;
        let done = handled(match desk {
            Desk::Buyer => self.buyer_mail(mail),
            Desk::Vendor(seller) => self.vendor_mail(&seller, mail),
            Desk::Moderator => self.moderator_mail(mail),
        })?;
        if done == Handled::Done {
            self.letter_guard.admit(&opened, &escrow, kind)?;
            self.letter_guard.pin(&opened, &escrow, kind)?;
        }
        Ok(done)
    }

    /// Open `letter` with the first of this node's mail keys that fits. A
    /// letter that opens but fails its signature is refused, not tried again.
    fn open_letter(&self, letter: &SealedLetter) -> Result<(Desk, Opened<Vec<u8>>), String> {
        let refuse = |err: tormail::Error| match err {
            tormail::Error::Unauthenticated => Err(err.to_string()),
            _ => Ok(()),
        };
        match open_message(letter, &self.buyer_mail_secret()) {
            Ok(opened) => return Ok((Desk::Buyer, opened)),
            Err(err) => refuse(err)?,
        }
        let mut sellers: Vec<String> = self.store_wallets.keys().cloned().collect();
        if let Some(name) = self.own_record().map(|store| store.name) {
            if !sellers.iter().any(|seller| seller == &name) {
                sellers.push(name);
            }
        }
        for seller in sellers {
            let Some(secret) = self.vendor_mail_secret(&seller) else {
                continue;
            };
            match open_message(letter, &secret) {
                Ok(opened) => return Ok((Desk::Vendor(seller), opened)),
                Err(err) => refuse(err)?,
            }
        }
        if self.escrow.config.moderates {
            // The built-in test moderator's mailbox, then this node's own.
            for secret in [Some(moderator_mail_secret()), self.own_moderator_mail_secret()].into_iter().flatten() {
                match open_message(letter, &secret) {
                    Ok(opened) => return Ok((Desk::Moderator, opened)),
                    Err(err) => refuse(err)?,
                }
            }
        }
        Err("no key on this node opens it".into())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Handled {
    Done,
    Deferred,
}

fn handled(result: Result<(), String>) -> Result<Handled, String> {
    match result {
        Ok(()) => Ok(Handled::Done),
        Err(err) if err.starts_with(RETRY_LATER) => Ok(Handled::Deferred),
        Err(err) => Err(err),
    }
}

/// Buyer rounds that sign or broadcast: countersigning terms, signing the
/// funding, signing the release again for a vendor's fresh nonce, and
/// settling a ruling.
fn buyer_step_signs(kind: Kind) -> bool {
    matches!(kind, Kind::Offer | Kind::Reply | Kind::ReleaseNonce | Kind::Ruling)
}

enum Desk {
    Buyer,
    Vendor(String),
    Moderator,
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
