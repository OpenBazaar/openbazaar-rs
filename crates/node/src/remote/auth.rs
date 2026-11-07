//! Signed requests from paired devices (protocol v2, "Signed requests").
//!
//! Every request names its device and carries a time, a nonce, and a
//! signature over the `OBRQ2` preimage ([`protocol::request_preimage`]),
//! so it cannot be moved to another node, method, path, or body. The node
//! takes a time within [`WINDOW_SECS`] of its clock and not before it
//! started, and each nonce once per device. Nonces are kept in memory for
//! twice the window; a restart forgets them, which is why a time from before
//! the restart is refused.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Keypair, Message, Secp256k1};

use super::protocol::{request_preimage, DeviceKey};
use crate::http_parse::header;
use crate::vault_key::encode_hex;

pub(crate) const WINDOW_SECS: u64 = 120;
const MAX_NONCES_PER_DEVICE: usize = 4096;
pub(crate) const DEVICE_HEADER: &str = "X-Ob-Device";
pub(crate) const TIME_HEADER: &str = "X-Ob-Time";
pub(crate) const NONCE_HEADER: &str = "X-Ob-Nonce";
pub(crate) const SIGNATURE_HEADER: &str = "X-Ob-Signature";

pub(crate) fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// A BIP 340 device key held by a node acting as a client of a home node.
pub(crate) struct Signer {
    keypair: Keypair,
}

impl Signer {
    pub(crate) fn from_secret(secret: &[u8; 32]) -> Option<Self> {
        Keypair::from_seckey_slice(&Secp256k1::new(), secret)
            .ok()
            .map(|keypair| Self { keypair })
    }

    pub(crate) fn key(&self) -> DeviceKey {
        DeviceKey::Bip340(self.keypair.x_only_public_key().0)
    }

    pub(crate) fn id(&self) -> String {
        self.key().id()
    }

    pub(crate) fn sign(&self, preimage: &[u8]) -> String {
        let aux = crate::vault_key::random_bytes::<32>();
        let digest = Message::from_digest(sha256::Hash::hash(preimage).to_byte_array());
        encode_hex(
            Secp256k1::new()
                .sign_schnorr_with_aux_rand(&digest, &self.keypair, &aux)
                .as_ref(),
        )
    }

    /// The four auth headers for one request, each ending in CRLF.
    pub(crate) fn headers(
        &self,
        node_id: &str,
        time: u64,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> String {
        let nonce = encode_hex(&crate::vault_key::random_bytes::<16>());
        let id = self.id();
        let signature =
            self.sign(request_preimage(node_id, &id, time, &nonce, method, path, body).as_bytes());
        format!("{DEVICE_HEADER}: {id}\r\n{TIME_HEADER}: {time}\r\n{NONCE_HEADER}: {nonce}\r\n{SIGNATURE_HEADER}: {signature}\r\n")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Outside the window, or before this process started.
    Clock,
    Replay,
}

/// The signed headers of one request, before the key is looked up.
pub(crate) struct Signed<'a> {
    pub device: &'a str,
    time: u64,
    nonce: &'a str,
    signature: &'a str,
}

impl<'a> Signed<'a> {
    pub(crate) fn read(head: &'a str) -> Option<Self> {
        let device = header(head, DEVICE_HEADER)?;
        let time = header(head, TIME_HEADER)?.parse().ok()?;
        let nonce = header(head, NONCE_HEADER)?;
        let signature = header(head, SIGNATURE_HEADER)?;
        let ok = device.len() == 16
            && device.bytes().all(|b| b.is_ascii_hexdigit())
            && nonce.len() == 32
            && nonce.bytes().all(|b| b.is_ascii_hexdigit());
        ok.then_some(Self {
            device,
            time,
            nonce,
            signature,
        })
    }

    pub(crate) fn verifies(
        &self,
        key: &DeviceKey,
        node_id: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> bool {
        let preimage = request_preimage(
            node_id,
            self.device,
            self.time,
            self.nonce,
            method,
            path,
            body,
        );
        key.verifies(preimage.as_bytes(), self.signature)
    }
}

/// Times and nonces already used, per device.
pub(crate) struct ReplayGuard {
    started: u64,
    seen: Mutex<HashMap<String, VecDeque<(u64, String)>>>,
}

impl ReplayGuard {
    pub(crate) fn new(started: u64) -> Self {
        Self {
            started,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Record the nonce of a request whose signature already verified.
    pub(crate) fn admit(&self, signed: &Signed<'_>, now: u64) -> Result<(), Refused> {
        if signed.time < self.started
            || signed.time + WINDOW_SECS < now
            || signed.time > now + WINDOW_SECS
        {
            return Err(Refused::Clock);
        }
        let mut seen = self.seen.lock().unwrap_or_else(|err| err.into_inner());
        let list = seen.entry(signed.device.to_string()).or_default();
        while list
            .front()
            .is_some_and(|(time, _)| time + 2 * WINDOW_SECS < now)
        {
            list.pop_front();
        }
        if list.iter().any(|(_, nonce)| nonce == signed.nonce)
            || list.len() >= MAX_NONCES_PER_DEVICE
        {
            return Err(Refused::Replay);
        }
        list.push_back((now.max(signed.time), signed.nonce.to_string()));
        Ok(())
    }

    pub(crate) fn forget(&self, device: &str) {
        self.seen
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(device);
    }
}
