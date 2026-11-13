//! The vendor's side of a moderated escrow (testnet4 only).
//!
//! A buyer's request names the buyer's per-order key and payout, and the
//! moderator it chose when that is not the built-in one. The vendor takes
//! the moderator only if the store's signed record accepts it
//! ([`crate::escrow_choice`]),
//! picks the heights from its own chain tip, adds a fresh per-order key from
//! the seller's store wallet seed (`m/1984'/coin'/1'/index'`), signs the
//! terms, and runs round 2 and the funding finish. Its key never leaves
//! this node and it never sees the buyer's.
//!
//! Every sale is a durable record ([`crate::escrow_store`]) holding the
//! terms, the round 2 snapshot, the funded escrow's snapshot and the claim.
//! Each step stores the record before it sends what it produced, and
//! answers a repeated message with what it stored, so a restart at any
//! round picks up where it was. The secret key is never stored; it is
//! derived again from the seed, so a restored sale needs the spend tier.
//! A release nonce lives only in memory: after a restart the vendor sends a
//! fresh one and the buyer signs again.

use std::collections::{BTreeMap, HashMap};

use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1};
use bitcoin::{Address, Amount, ScriptBuf, Transaction};
use openbazaar_escrow::{
    BuyerOpen, BuyerSignatures, EscrowTerms, ReleaseSignature, SignedTerms, VendorSession,
};
use serde_json::{json, Value};

use crate::escrow_buyer::read_attempt;
use crate::escrow_mail::{hex, id_hex, key_bytes, EscrowId, Kind, Mail, RETRY_LATER};
use crate::network::NetworkMode;
use crate::Node;

mod dispute;
mod sale;

use sale::{Live, Sale};

/// Blocks from the vendor's tip to the dispute deadline (about two weeks).
pub(crate) const DELIVERY_BLOCKS: u32 = 2_016;
/// Blocks from the dispute deadline to the vendor claim, and from there to the refund.
pub(crate) const RULING_BLOCKS: u32 = 144;
pub(crate) const CLAIM_BLOCKS: u32 = 144;

const OFFER_TAG: &[u8] = b"ob-node/escrow-offer/v1";

/// What the seller's store key signs, binding the terms to this escrow id.
pub(crate) fn offer_digest(id: &EscrowId, terms: &EscrowTerms) -> Message {
    let mut engine = sha256::Hash::engine();
    engine.input(OFFER_TAG);
    engine.input(id);
    engine.input(&terms.digest());
    Message::from_digest(sha256::Hash::from_engine(engine).to_byte_array())
}

const OFFER_LISTING_TAG: &[u8] = b"ob-node/escrow-offer/v2";

/// The store signature once the offer also names the listing.
pub(crate) fn offer_digest_with_listing(id: &EscrowId, terms: &EscrowTerms, listing_id: &[u8; 16]) -> Message {
    let mut engine = sha256::Hash::engine();
    engine.input(OFFER_LISTING_TAG);
    engine.input(id);
    engine.input(&terms.digest());
    engine.input(listing_id);
    Message::from_digest(sha256::Hash::from_engine(engine).to_byte_array())
}

/// Sales from disk, and the funded escrows restored in this process.
#[derive(Default)]
pub(crate) struct VendorDesk {
    sales: BTreeMap<EscrowId, Sale>,
    live: HashMap<EscrowId, Live>,
}

impl VendorDesk {
    fn value(&self, network: NetworkMode) -> Value {
        let address = |script: &ScriptBuf| {
            Address::from_script(script, network.bitcoin())
                .map(|address| address.to_string())
                .ok()
        };
        let sales: Vec<Value> = self
            .sales
            .iter()
            .map(|(id, sale)| {
                json!({
                    "id": id_hex(id),
                    "seller": sale.seller,
                    "stage": sale.stage,
                    "fundingSats": sale.terms.funding_value.to_sat(),
                    "networkFeeSats": sale.terms.network_fee.to_sat(),
                    "address": sale.terms.funding_output().ok().and_then(|out| address(&out.script_pubkey)),
                    "outpoint": sale.outpoint.map(|outpoint| outpoint.to_string()),
                    "vendorKeyIndex": sale.index,
                    "vendorPayout": address(&sale.terms.vendor_payout),
                    "disputeDeadline": sale.terms.dispute_deadline,
                    "vendorClaimHeight": sale.terms.vendor_claim_height,
                    "refundHeight": sale.terms.refund_height,
                    "claimHeld": sale.claim.is_some(),
                    "claim": sale.claim.as_deref().map(hex),
                    "releaseTxid": sale.release_txid.map(|txid| txid.to_string()),
                    "settleTxid": sale.settle_txid.map(|txid| txid.to_string()),
                    "error": sale.error,
                })
            })
            .collect();
        json!({ "sales": sales })
    }
}

fn failed(err: impl std::fmt::Display) -> String {
    err.to_string()
}

impl Node {
    pub(crate) fn sales_value(&self) -> Value {
        self.vendor_desk().value(self.network)
    }

    pub(crate) fn vendor_escrow_ids(&self) -> Vec<EscrowId> {
        self.vendor_desk().sales.keys().copied().collect()
    }

    fn vendor_desk(&self) -> std::sync::MutexGuard<'_, VendorDesk> {
        self.escrow
            .vendor
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn vendor_secret(&self, seller: &str, index: u32) -> Result<[u8; 32], String> {
        let wallet = self.store_wallet(seller).map_err(failed)?;
        Ok(self.escrow_secret(&wallet, index)?.secret_bytes())
    }

    /// The sale for `id`, checked to belong to `seller`, with what `read` takes from it.
    fn sale_of<T>(&self, seller: &str, id: &EscrowId, read: impl FnOnce(&Sale) -> T) -> Result<T, String> {
        let desk = self.vendor_desk();
        let sale = desk.sales.get(id).ok_or("no sale for that escrow")?;
        if sale.seller != seller {
            return Err("that sale belongs to another seller".into());
        }
        Ok(read(sale))
    }

    fn send_to_buyer(&self, id: &EscrowId, kind: Kind, fields: Vec<Vec<u8>>) -> Result<(), String> {
        let (url, to) = {
            let desk = self.vendor_desk();
            let sale = desk.sales.get(id).ok_or("no sale for that escrow")?;
            (sale.buyer_url.clone(), sale.buyer_mail)
        };
        self.send_mail(&url, &to, &Mail::new(kind, *id, fields)).map(|_| ())
    }

    pub(crate) fn vendor_mail(&self, seller: &str, mail: Mail) -> Result<(), String> {
        match mail.kind {
            Kind::Request => self.vendor_request(seller, &mail),
            Kind::Open => self.vendor_open(seller, &mail),
            Kind::Signatures => self.vendor_signatures(seller, &mail),
            Kind::Release => self.vendor_release(seller, &mail),
            Kind::Ruling => self.vendor_ruling(seller, &mail),
            other => Err(format!("a vendor does not take {other:?}")),
        }
    }

    fn seller_has_listing(&self, seller: &str, id: &[u8; 16]) -> bool {
        let id = openbazaar_protocol::ListingId(*id);
        if self.own_listings().by_id(&id).is_some() {
            return true;
        }
        let Ok(store) = self.seller_store(seller) else {
            return false;
        };
        self.catalog().listings.iter().any(|listing| listing.id() == Some(id) && listing.store_id == store.id)
    }

    pub(crate) fn finished_escrow_sales(&self) -> u32 {
        let desk = self.vendor_desk();
        crate::reputation::finished_escrow_count(
            &desk
                .sales
                .values()
                .map(|sale| crate::reputation::Finish {
                    payout: sale.release_txid.is_some() || sale.settle_txid.is_some(),
                    self_pay: sale.terms.buyer_payout == sale.terms.vendor_payout,
                    direct: false,
                })
                .collect::<Vec<_>>(),
        )
    }

    /// Round 0: answer a request with terms signed by the vendor's per-order key.
    fn vendor_request(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.as_fields();
        if !(6..=8).contains(&fields.len()) {
            return Err("a Request message carries 6 fields".into());
        }
        let buyer: [u8; 33] = fields[0]
            .as_slice()
            .try_into()
            .map_err(|_| "bad buyer key")?;
        let buyer_payout = ScriptBuf::from_bytes(fields[1].clone());
        let buyer_mail = key_bytes(&fields[2])?;
        let buyer_url = crate::peer_http::sale_base(&String::from_utf8(fields[3].clone()).map_err(failed)?)?;
        if self.own_url() == Some(buyer_url.as_str()) || self.sale_reply_url().as_deref() == Some(buyer_url.as_str()) {
            return Err("this node will not sell to itself".into());
        }
        let funding: [u8; 8] = fields[4]
            .as_slice()
            .try_into()
            .map_err(|_| "bad funding amount")?;
        let fee: [u8; 8] = fields[5]
            .as_slice()
            .try_into()
            .map_err(|_| "bad network fee")?;
        // The listing field is empty when the buyer names a moderator but no listing.
        let listing_id = match fields.get(6).filter(|field| !field.is_empty()) {
            Some(field) => {
                let id: [u8; 16] = field.as_slice().try_into().map_err(|_| "bad listing id")?;
                if !self.seller_has_listing(seller, &id) {
                    return Err("that listing is not this store's".into());
                }
                Some(id)
            }
            None => None,
        };
        let chosen = match fields.get(7) {
            Some(field) => Some(<[u8; 32]>::try_from(field.as_slice()).map_err(|_| "bad moderator id")?),
            None => None,
        };
        if let Some(offer) = self.repeated_request(seller, &mail.escrow, &buyer)? {
            return self.send_to_buyer(&mail.escrow, Kind::Offer, offer);
        }
        self.ensure_testnet_bond();
        if let Err(err) = crate::reputation::escrow_start_allowed(self.finished_escrow_sales(), self.passing_bond()) {
            return Err(format!("{RETRY_LATER}{err}"));
        }
        let ctx = self.escrow_context(self.sale_moderator(seller, chosen)?)?;
        let (index, secret, payout) = {
            let mut wallet = self.store_wallet(seller).map_err(failed)?;
            let index = wallet.next_escrow_index().map_err(failed)?;
            let secret = self.escrow_secret(&wallet, index)?;
            (index, secret, wallet.fresh_address().map_err(failed)?)
        };
        let dispute_deadline = ctx
            .tip
            .checked_add(DELIVERY_BLOCKS)
            .ok_or("tip out of range")?;
        let terms = EscrowTerms {
            buyer,
            vendor: PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize(),
            moderator: ctx.moderator.clone(),
            funding_value: Amount::from_sat(u64::from_le_bytes(funding)),
            // The buyer's pick; validate() holds it between the policy's floor and cap.
            network_fee: Amount::from_sat(u64::from_le_bytes(fee)),
            dispute_deadline,
            vendor_claim_height: dispute_deadline + RULING_BLOCKS,
            refund_height: dispute_deadline + RULING_BLOCKS + CLAIM_BLOCKS,
            buyer_payout,
            vendor_payout: payout.script_pubkey(),
        };
        terms.validate(&ctx.policy).map_err(failed)?;
        terms
            .validate_at_tip(&ctx.policy, ctx.tip)
            .map_err(failed)?;
        let vendor_signature = terms.sign(&secret.secret_bytes()).map_err(failed)?;
        let store = self.seller_store_secret(seller)?;
        let secp = Secp256k1::new();
        let digest = match listing_id {
            Some(id) => offer_digest_with_listing(&mail.escrow, &terms, &id),
            None => offer_digest(&mail.escrow, &terms),
        };
        let store_signature = secp
            .sign_schnorr_no_aux_rand(&digest, &Keypair::from_secret_key(&secp, &store))
            .serialize();
        let mut offer = vec![
            terms.encode(),
            vendor_signature.to_vec(),
            store_signature.to_vec(),
        ];
        if let Some(id) = listing_id {
            offer.push(id.to_vec());
        }
        self.save_sale(
            &mail.escrow,
            Sale {
                seller: seller.to_string(),
                index,
                terms,
                buyer_mail,
                buyer_url,
                stage: "offered".into(),
                offer: offer.clone(),
                outpoint: None,
                open_attempt: 0,
                reply: None,
                awaiting: None,
                escrow: None,
                claim: None,
                outcome_tx: None,
                release_txid: None,
                settle_txid: None,
                error: None,
                listing_id,
            },
        )?;
        self.send_to_buyer(&mail.escrow, Kind::Offer, offer)
    }

    /// The stored offer when this escrow already has terms for the same buyer key.
    fn repeated_request(&self, seller: &str, id: &EscrowId, buyer: &[u8; 33]) -> Result<Option<Vec<Vec<u8>>>, String> {
        let desk = self.vendor_desk();
        let Some(sale) = desk.sales.get(id) else {
            return Ok(None);
        };
        if sale.seller != seller || &sale.terms.buyer != buyer {
            return Err("that escrow already has terms".into());
        }
        if sale.stage != "offered" {
            return Err(format!("that sale is {}", sale.stage));
        }
        Ok(Some(sale.offer.clone()))
    }

    /// Round 2: check the buyer's opening and sign the vendor's half.
    ///
    /// A buyer that lost its round 1 session opens again with a higher
    /// attempt (fresh nonces on its own side); the vendor answers each new
    /// attempt with fresh nonces of its own and replaces the stored round.
    /// The same attempt again gets the stored reply.
    fn vendor_open(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.fields(3)?;
        let attempt = read_attempt(&fields[2])?;
        let (stage, stored_attempt, stored_reply, terms, index) = self.sale_of(seller, &mail.escrow, |sale| {
            (sale.stage.clone(), sale.open_attempt, sale.reply.clone(), sale.terms.clone(), sale.index)
        })?;
        match stage.as_str() {
            "offered" => {}
            "replied" if attempt == stored_attempt => {
                let reply = stored_reply.ok_or("the stored reply is missing")?;
                return self.send_to_buyer(&mail.escrow, Kind::Reply, vec![reply, fields[2].clone()]);
            }
            "replied" if attempt > stored_attempt => {}
            "replied" => return Err(format!("open attempt {attempt} is older than {stored_attempt}")),
            other => return Err(format!("that sale is {other}")),
        }
        let signed = SignedTerms::decode(&fields[0]).map_err(failed)?;
        let open = BuyerOpen::from_bytes(&fields[1]).map_err(failed)?;
        if signed.terms() != &terms {
            return Err("the buyer signed different terms".into());
        }
        let secret = self.vendor_secret(seller, index)?;
        let ctx = self.escrow_context(terms.moderator.clone())?;
        let (awaiting, reply) =
            VendorSession::accept(signed, &secret, &ctx, &open).map_err(failed)?;
        let reply = reply.to_bytes();
        self.update_sale(&mail.escrow, |sale| {
            sale.stage = "replied".into();
            sale.outpoint = Some(open.funding_outpoint);
            sale.open_attempt = attempt;
            sale.reply = Some(reply.clone());
            sale.awaiting = Some(awaiting.snapshot());
        })?;
        self.send_to_buyer(&mail.escrow, Kind::Reply, vec![reply, fields[2].clone()])
    }

    /// Funding finish: store the claim and the funded escrow, then offer a
    /// release nonce. Signatures again (the buyer saw no nonce) get a fresh one.
    fn vendor_signatures(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.fields(1)?;
        let signatures = BuyerSignatures::from_bytes(&fields[0]).map_err(failed)?;
        match self.sale_of(seller, &mail.escrow, |sale| sale.stage.clone())?.as_str() {
            "replied" => {}
            "funded" => return self.send_release_nonce(&mail.escrow),
            other => return Err(format!("that sale is {other}")),
        }
        let awaiting = self.restore_awaiting(&mail.escrow)?;
        // A failure leaves the stored round as it was: the buyer may still
        // send the signatures for it.
        let escrow = awaiting.receive(&signatures).map_err(failed)?;
        let (claim, snapshot) = (escrow.claim().to_bytes(), escrow.snapshot());
        self.update_sale(&mail.escrow, |sale| {
            sale.stage = "funded".into();
            sale.claim = Some(claim);
            sale.escrow = Some(snapshot);
            sale.awaiting = None;
            sale.reply = None;
        })?;
        self.vendor_desk().live.insert(mail.escrow, Live { escrow, nonce: None });
        self.send_release_nonce(&mail.escrow)
    }

    /// Make a fresh release nonce, replacing any held one, and send it.
    fn send_release_nonce(&self, id: &EscrowId) -> Result<(), String> {
        let public = self.with_vendor_escrow(
            id,
            |live| {
                let nonce = live.escrow.release_nonce().map_err(failed)?;
                let public = nonce.public().serialize().to_vec();
                live.nonce = Some(nonce);
                Ok(public)
            },
            |_, _| {},
        )?;
        self.send_to_buyer(id, Kind::ReleaseNonce, vec![public])
    }

    /// Happy path: finish the release with the buyer's signature and
    /// broadcast it. Without a matching nonce (the vendor restarted, or
    /// the buyer signed an older one) the buyer gets a fresh nonce instead.
    fn vendor_release(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.fields(1)?;
        let signature = ReleaseSignature::from_bytes(&fields[0]).map_err(failed)?;
        match self.sale_of(seller, &mail.escrow, |sale| sale.stage.clone())?.as_str() {
            "funded" => {}
            "released" => return self.broadcast_outcome(&mail.escrow),
            other => return Err(format!("that sale is {other}")),
        }
        let done = self.with_vendor_escrow(
            &mail.escrow,
            |live| match live.nonce.take() {
                Some(nonce) => Ok(live.escrow.complete_release(nonce, &signature).ok()),
                None => Ok(None),
            },
            |sale, tx| {
                if let Some(tx) = tx {
                    sale.stage = "released".into();
                    sale.outcome_tx = Some(serialize_hex(tx));
                    sale.release_txid = Some(tx.compute_txid());
                }
            },
        )?;
        if done.is_none() {
            eprintln!("ob-node: escrow {}: the release does not fit a held nonce; sending a fresh one", id_hex(&mail.escrow));
            return self.send_release_nonce(&mail.escrow);
        }
        self.broadcast_outcome(&mail.escrow)
    }

    /// Broadcast the stored release or dispute outcome.
    fn broadcast_outcome(&self, id: &EscrowId) -> Result<(), String> {
        let stored = self.vendor_desk().sales.get(id).and_then(|sale| sale.outcome_tx.clone());
        let Some(stored) = stored else {
            return Ok(());
        };
        let tx: Transaction = deserialize_hex(&stored).map_err(failed)?;
        let result = self.chain.broadcast(&tx);
        self.update_sale(id, |sale| {
            sale.error = result
                .as_ref()
                .err()
                .map(|err| format!("The outcome {} is signed but not broadcast: {err}", tx.compute_txid()));
        })?;
        result
    }
}
