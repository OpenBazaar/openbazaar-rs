//! BIP 77 Payjoin v2 sessions.
//!
//! The sender and the receiver are the `payjoin` crate's v2 typestates. The
//! oblivious directory is a URL the node already has. This module does not
//! speak a new mailbox protocol, open a socket, or fetch OHTTP keys. The node
//! passes in the directory URL and the key configuration it fetched.
//!
//! Once the directory has delivered the buyer's original PSBT, the vendor adds
//! one of their own inputs. [`contribute_vendor_input`] runs that step.
//! `payjoin::receive::v2::Receiver<WantsInputs>::contribute_inputs` delegates to
//! the same routine. Reaching `WantsInputs` requires the directory's OHTTP
//! response, so the function starts from the original PSBT and does not
//! contact the network. Signing the proposal and broadcasting it belong to the
//! wallet in the node crate.

use std::fmt;
use std::sync::Mutex;

use ::payjoin::persist::SessionPersister;
use ::payjoin::receive::v1::{self, Headers};
use ::payjoin::receive::v2::{self as receive_v2, Receiver};
use ::payjoin::receive::InputPair;
use ::payjoin::send::v2::{self as send_v2, Sender, SenderBuilder};
use ::payjoin::{OhttpKeys, OhttpResponse};
use bitcoin::psbt::Psbt;
use bitcoin::{Address, Amount, FeeRate, OutPoint, Script};

use crate::error::{payjoin, PayError};

/// Store-and-forward Payjoin directory (BIP 77). Configuration only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObliviousDirectory {
    url: String,
}

impl ObliviousDirectory {
    /// Accept an `http` or `https` directory URL.
    pub fn new(url: &str) -> Result<Self, PayError> {
        let parsed =
            ::payjoin::Url::parse(url).map_err(|err| PayError::Directory(err.to_string()))?;
        match parsed.scheme() {
            "http" | "https" => Ok(Self {
                url: parsed.to_string(),
            }),
            other => Err(PayError::Directory(format!(
                "payjoin directory scheme must be http or https, got {other}"
            ))),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.url
    }
}

/// A BIP 77 receiver session waiting for the buyer's original PSBT.
pub struct PayjoinReceiver {
    address: Address,
    inner: Receiver<receive_v2::Initialized>,
    /// Same event log `open` wrote, kept for a later directory response.
    #[allow(dead_code)]
    persister: EventLog<receive_v2::SessionEvent>,
}

impl PayjoinReceiver {
    /// Open a receiver session for `address` at `directory`.
    ///
    /// `ohttp_key_config` is the directory's OHTTP key configuration, already
    /// fetched by the node. `price` is written into the BIP 21 URI.
    pub fn open(
        address: Address,
        directory: &ObliviousDirectory,
        ohttp_key_config: &[u8],
        price: Amount,
    ) -> Result<Self, PayError> {
        if price.to_sat() == 0 {
            return Err(PayError::Payjoin(
                "payjoin price must be at least one satoshi".into(),
            ));
        }
        let keys = OhttpKeys::decode(ohttp_key_config).map_err(payjoin)?;
        let persister = EventLog::default();
        let inner = receive_v2::ReceiverBuilder::new(address.clone(), directory.as_str(), keys)
            .map_err(payjoin)?
            .with_amount(price)
            .build()
            .save(&persister)
            .map_err(payjoin)?;
        Ok(Self {
            address,
            inner,
            persister,
        })
    }

    pub fn address(&self) -> &Address {
        &self.address
    }

    /// BIP 21 URI carrying the v2 `pj` parameter.
    pub fn payjoin_uri(&self) -> String {
        self.inner.pj_uri().to_string()
    }

    /// OHTTP GET the node sends to `ohttp_relay` to poll the directory.
    pub fn poll_for_original_psbt(
        &self,
        ohttp_relay: &str,
    ) -> Result<EncapsulatedRequest, PayError> {
        let (request, context) = self
            .inner
            .create_poll_request(ohttp_relay)
            .map_err(payjoin)?;
        Ok(EncapsulatedRequest::from_payjoin(request, context))
    }
}

impl fmt::Debug for PayjoinReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayjoinReceiver")
            .field("address", &self.address)
            .field("payjoin_uri", &self.payjoin_uri())
            .finish_non_exhaustive()
    }
}

/// A BIP 77 sender session holding the buyer's original PSBT.
pub struct PayjoinSender {
    inner: Sender<send_v2::WithReplyKey>,
    /// Same event log `open` wrote, kept for a later directory response.
    #[allow(dead_code)]
    persister: EventLog<send_v2::SessionEvent>,
}

impl PayjoinSender {
    /// Build a sender that pays `receiver` with `original`.
    ///
    /// `min_fee_rate` is the floor the sender puts on the original transaction.
    /// The original PSBT has to pay the receiver address.
    pub fn open(
        original: Psbt,
        receiver: &PayjoinReceiver,
        min_fee_rate: FeeRate,
    ) -> Result<Self, PayError> {
        let uri = receiver.inner.pj_uri();
        let ::payjoin::PjParam::V2(pj_param) = uri.extras().pj_param() else {
            return Err(PayError::Payjoin(
                "receiver session produced a BIP 78 endpoint".into(),
            ));
        };
        let persister = EventLog::default();
        let inner = SenderBuilder::from_parts(original, pj_param, uri.address(), uri.amount())
            .build_non_incentivizing(min_fee_rate)
            .map_err(payjoin)?
            .save(&persister)
            .map_err(payjoin)?;
        Ok(Self { inner, persister })
    }

    /// OHTTP POST the node sends to `ohttp_relay` with the original PSBT.
    pub fn post_original_psbt(&self, ohttp_relay: &str) -> Result<EncapsulatedRequest, PayError> {
        let (request, context) = self
            .inner
            .create_v2_post_request(ohttp_relay)
            .map_err(payjoin)?;
        Ok(EncapsulatedRequest::from_payjoin(request, context))
    }
}

impl fmt::Debug for PayjoinSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayjoinSender")
            .field("endpoint", &self.inner.endpoint())
            .finish_non_exhaustive()
    }
}

/// An OHTTP request the node transmits. The response context stays with it
/// so a later call can decapsulate the directory's reply.
pub struct EncapsulatedRequest {
    pub url: String,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// OHTTP response context paired with `body`. The node keeps this request
    /// and feeds the directory reply back through the payjoin session.
    #[allow(dead_code)]
    context: OhttpResponse,
}

impl EncapsulatedRequest {
    fn from_payjoin(request: ::payjoin::Request, context: OhttpResponse) -> Self {
        Self {
            url: request.url,
            content_type: request.content_type,
            body: request.body,
            context,
        }
    }
}

impl fmt::Debug for EncapsulatedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncapsulatedRequest")
            .field("url", &self.url)
            .field("content_type", &self.content_type)
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// Original PSBT after the vendor has added an input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContributedProposal {
    psbt: Psbt,
}

impl ContributedProposal {
    pub fn psbt(&self) -> &Psbt {
        &self.psbt
    }

    pub fn input_outpoints(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.psbt
            .unsigned_tx
            .input
            .iter()
            .map(|input| input.previous_output)
    }

    pub fn output_value(&self, script: &Script) -> Option<Amount> {
        self.psbt
            .unsigned_tx
            .output
            .iter()
            .find(|output| output.script_pubkey.as_script() == script)
            .map(|output| output.value)
    }
}

/// Add the vendor's input to the buyer's original PSBT.
///
/// The vendor output that `receiver_script` identifies absorbs the extra value,
/// which is the Payjoin property that the vendor contributed coins. The returned
/// PSBT is ready for the vendor's wallet to sign. This function does not broadcast.
pub fn contribute_vendor_input(
    original: &Psbt,
    receiver_script: &Script,
    vendor_input: InputPair,
) -> Result<ContributedProposal, PayError> {
    let body = original.to_string();
    let headers = RequestHeaders {
        content_type: "text/plain",
        content_length: body.len().to_string(),
    };
    let wants_outputs = v1::UncheckedOriginalPayload::from_request(body.as_bytes(), "v=1", headers)
        .map_err(payjoin)?
        .assume_interactive_receiver()
        .check_inputs_not_owned(&mut |_| Ok(false))
        .map_err(payjoin)?
        .check_no_inputs_seen_before(&mut |_| Ok(false))
        .map_err(payjoin)?
        .identify_receiver_outputs(&mut |script| Ok(script == receiver_script))
        .map_err(payjoin)?;
    let provisional = wants_outputs
        .commit_outputs()
        .contribute_inputs([vendor_input])
        .map_err(payjoin)?
        .commit_inputs()
        .apply_fee_range(None, Some(FeeRate::BROADCAST_MIN))
        .map_err(payjoin)?;
    Ok(ContributedProposal {
        psbt: provisional.psbt_to_sign(),
    })
}

struct RequestHeaders {
    content_type: &'static str,
    content_length: String,
}

impl Headers for RequestHeaders {
    fn get_header(&self, key: &str) -> Option<&str> {
        if key.eq_ignore_ascii_case("content-type") {
            Some(self.content_type)
        } else if key.eq_ignore_ascii_case("content-length") {
            Some(&self.content_length)
        } else {
            None
        }
    }
}

#[derive(Debug)]
struct EventLog<E> {
    events: Mutex<Vec<E>>,
}

impl<E> Default for EventLog<E> {
    fn default() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }
}

impl<E: Clone + Send + 'static> SessionPersister for EventLog<E> {
    type InternalStorageError = std::io::Error;
    type SessionEvent = E;

    fn save_event(&self, event: Self::SessionEvent) -> Result<(), Self::InternalStorageError> {
        self.events.lock().map_err(poisoned)?.push(event);
        Ok(())
    }

    fn load(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Self::SessionEvent>>, Self::InternalStorageError> {
        let events = self.events.lock().map_err(poisoned)?.clone();
        Ok(Box::new(events.into_iter()))
    }

    fn close(&self) -> Result<(), Self::InternalStorageError> {
        Ok(())
    }
}

fn poisoned(err: impl fmt::Display) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

#[cfg(test)]
mod tests {
    use bitcoin::absolute::LockTime;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::transaction::Version;
    use bitcoin::{
        Address, Amount, CompressedPublicKey, Network, OutPoint, ScriptBuf, Sequence, Transaction,
        TxIn, TxOut, Witness,
    };

    use super::*;

    #[test]
    fn receiver_and_sender_build_local_requests() {
        let directory = ObliviousDirectory::new("http://127.0.0.1:8090").unwrap();
        let receiver_address = p2wpkh(2);
        let receiver = PayjoinReceiver::open(
            receiver_address.clone(),
            &directory,
            &ohttp_key_config(),
            Amount::from_sat(50_000),
        )
        .unwrap();
        let uri = receiver.payjoin_uri();
        assert!(uri.starts_with("bitcoin:"));
        assert!(uri.contains("pj="));
        assert!(uri.contains(directory.as_str()) || uri.contains("127.0.0.1"));

        let poll = receiver
            .poll_for_original_psbt("http://127.0.0.1:8091")
            .unwrap();
        assert!(poll.url.contains("127.0.0.1:8091"));
        assert!(!poll.body.is_empty());
        assert_eq!(poll.content_type, "message/ohttp-req");

        let (original, _vendor_outpoint, _vendor_txout) = original_payment(&receiver_address);
        let sender =
            PayjoinSender::open(original, &receiver, FeeRate::from_sat_per_kwu(250)).unwrap();
        let post = sender.post_original_psbt("http://127.0.0.1:8091").unwrap();
        assert!(post.url.contains("127.0.0.1:8091"));
        assert!(!post.body.is_empty());
        assert_eq!(post.content_type, "message/ohttp-req");
    }

    #[test]
    fn vendor_input_is_added_without_the_network() {
        let receiver_address = p2wpkh(2);
        let (original, vendor_outpoint, vendor_txout) = original_payment(&receiver_address);
        let script = receiver_address.script_pubkey();
        let before = script_value(&original, script.as_script()).unwrap();
        let vendor = InputPair::new_p2wpkh(vendor_txout, vendor_outpoint).unwrap();

        let proposal = contribute_vendor_input(&original, script.as_script(), vendor).unwrap();

        assert!(proposal
            .input_outpoints()
            .any(|outpoint| outpoint == vendor_outpoint));
        assert_eq!(proposal.input_outpoints().count(), 2);
        let after = proposal.output_value(script.as_script()).unwrap();
        assert!(after > before);
        assert!(after <= before + Amount::from_sat(40_000));
    }

    #[test]
    fn contribution_rejects_a_psbt_that_pays_someone_else() {
        let receiver_address = p2wpkh(2);
        let (original, vendor_outpoint, vendor_txout) = original_payment(&receiver_address);
        let stranger = p2wpkh(9).script_pubkey();
        let vendor = InputPair::new_p2wpkh(vendor_txout, vendor_outpoint).unwrap();
        assert!(contribute_vendor_input(&original, stranger.as_script(), vendor).is_err());
    }

    #[test]
    fn rejects_a_bad_directory_and_bad_ohttp_keys() {
        assert!(ObliviousDirectory::new("not a url").is_err());
        assert!(ObliviousDirectory::new("ftp://127.0.0.1/pj").is_err());
        let directory = ObliviousDirectory::new("https://directory.example").unwrap();
        let error =
            PayjoinReceiver::open(p2wpkh(2), &directory, b"not-keys", Amount::from_sat(1_000));
        assert!(error.is_err());
    }

    fn script_value(psbt: &Psbt, script: &bitcoin::Script) -> Option<Amount> {
        psbt.unsigned_tx
            .output
            .iter()
            .find(|output| output.script_pubkey.as_script() == script)
            .map(|output| output.value)
    }

    fn original_payment(receiver: &Address) -> (Psbt, OutPoint, TxOut) {
        let sender = p2wpkh(3);
        let (sender_outpoint, sender_txout) =
            utxo(Amount::from_sat(100_000), sender.script_pubkey());
        let (vendor_outpoint, vendor_txout) =
            utxo(Amount::from_sat(40_000), p2wpkh(4).script_pubkey());
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: sender_outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::default(),
            }],
            output: vec![
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: receiver.script_pubkey(),
                },
                TxOut {
                    value: Amount::from_sat(49_000),
                    script_pubkey: sender.script_pubkey(),
                },
            ],
        };
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        psbt.inputs[0].witness_utxo = Some(sender_txout);
        (psbt, vendor_outpoint, vendor_txout)
    }

    fn utxo(value: Amount, script_pubkey: ScriptBuf) -> (OutPoint, TxOut) {
        let prev = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value,
                script_pubkey,
            }],
        };
        let txout = prev.output[0].clone();
        let outpoint = OutPoint {
            txid: prev.compute_txid(),
            vout: 0,
        };
        (outpoint, txout)
    }

    fn p2wpkh(byte: u8) -> Address {
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[byte; 32]).unwrap();
        let public = bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &secret);
        let compressed = CompressedPublicKey::from_slice(&public.serialize()).unwrap();
        Address::p2wpkh(&compressed, Network::Regtest)
    }

    fn ohttp_key_config() -> Vec<u8> {
        use bitcoin_ohttp::hpke::{Aead, Kdf, Kem};
        use bitcoin_ohttp::{KeyConfig, SymmetricSuite};
        let config = KeyConfig::new(
            1,
            Kem::K256Sha256,
            vec![SymmetricSuite::new(Kdf::HkdfSha256, Aead::ChaCha20Poly1305)],
        )
        .expect("ohttp key config");
        config.encode().expect("ohttp key config bytes")
    }
}
