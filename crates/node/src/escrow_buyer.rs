//! The buyer's side of a moderated escrow (testnet4 only).
//!
//! ```text
//! checkout         -> Request (buyer key, payout, mail key)     to vendor
//! Offer            <- terms signed by the vendor and its store key
//! POST /pay        -> BuyerSession::start on an unsigned PSBT, Open to vendor
//! Reply            <- receive: refund held, then sign, send Signatures, broadcast
//! ReleaseNonce     <- kept for POST /orders/{id}/release
//! POST /release    -> ReleaseSignature to vendor
//! POST /dispute    -> dispute letter to the moderator
//! Ruling           <- settle_dispute, broadcast the buyer-wins outcome
//! ```
//!
//! The buyer's per-order key is `m/1984'/coin'/1'/index'` from this node's
//! seed. The refund is stored in the order record before the funding is
//! signed. The live [`BuyerEscrow`] is stored as a snapshot
//! ([`crate::escrow_store`]) before anything it signs is sent, and is
//! restored on demand with the per-order key (so only while unlocked).
//!
//! Round 1 holds secret nonces, which are never stored. A buyer that
//! restarts between `Open` and `Reply` opens again from the stored funding
//! PSBT with fresh nonces and the next attempt number; a reply to an older
//! attempt is dropped. The PSBT's inputs are locked from `Open` until the
//! funding is broadcast ([`crate::coin_locks`]).

use std::collections::HashMap;
use std::sync::MutexGuard;

use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{Address, Amount, Psbt, Transaction};
use musig2::PubNonce;
use openbazaar_escrow::{
    BuyerAwaitingReply, BuyerEscrow, BuyerSession, EscrowTerms, FundingPlan, ModeratorRuling,
    SignedTerms, VendorReply,
};
use serde_json::{json, Map, Value};

use crate::buyer_orders::BuyerOrder;
use crate::coin_locks::OPEN_LOCK_TTL;
use crate::escrow_fixture::{moderator_mail_key, moderator_record, MODERATOR_FEE_SATS};
use crate::escrow_mail::{hex, id_hex, parse_id, unhex, EscrowId, Kind, Mail};
use crate::escrow_outbox::{now_secs, Sent};
use crate::escrow_vendor::{offer_digest, offer_digest_with_listing};
use crate::instructions::{ESCROW_DEMO_WARNING, ESCROW_MAINNET_REFUSAL};
use crate::network::{address_for, NetworkMode};
use crate::pay_order::{check_fee, check_terms, psbt_fee, PayError, TERMS_CHANGED};
use crate::spend_tickets::SpendTerms;
use crate::watch::{AWAITING_FUNDING, DISPUTED, FUNDED, RELEASED, SETTLED};
use crate::{Node, OrderError};

mod state;

const MAX_MEMO: usize = 1_000;

/// Round 1 sessions and restored escrows. Only memory; the record and the
/// snapshot on disk are the truth.
#[derive(Default)]
pub(crate) struct BuyerDesk {
    waiting: HashMap<EscrowId, BuyerAwaitingReply>,
    escrows: HashMap<EscrowId, BuyerEscrow>,
}

fn attempt_bytes(attempt: u32) -> Vec<u8> {
    attempt.to_le_bytes().to_vec()
}

pub(crate) fn read_attempt(bytes: &[u8]) -> Result<u32, String> {
    let bytes: [u8; 4] = bytes.try_into().map_err(|_| "bad attempt number")?;
    Ok(u32::from_le_bytes(bytes))
}

fn text<'a>(escrow: &'a Value, key: &str) -> Result<&'a str, String> {
    escrow
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("this escrow has no {key}"))
}

fn number(escrow: &Value, key: &str) -> Result<u64, String> {
    escrow
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("this escrow has no {key}"))
}

pub(crate) fn escrow_id_of(escrow: &Value) -> Option<EscrowId> {
    escrow_id(escrow).ok()
}

fn escrow_id(escrow: &Value) -> Result<EscrowId, String> {
    parse_id(text(escrow, "id")?).ok_or_else(|| "this escrow id is not valid".into())
}

fn key_index(escrow: &Value) -> Result<u32, String> {
    u32::try_from(number(escrow, "buyerKeyIndex")?).map_err(|_| "bad key index".to_string())
}

fn failed(err: impl std::fmt::Display) -> String {
    err.to_string()
}

impl Node {
    fn buyer_desk(&self) -> MutexGuard<'_, crate::escrow_buyer::BuyerDesk> {
        self.escrow
            .buyer
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    /// The node that holds `seller`'s escrow keys. Refuses this node itself.
    pub(crate) fn vendor_url(&self, seller: &str) -> Result<String, String> {
        let url = self
            .escrow
            .config
            .vendors
            .get(seller)
            .ok_or_else(|| format!("No node for {seller} is set up to take escrow."))?;
        if self.own_url() == Some(url.trim_end_matches('/')) || self.sale_reply_url().as_deref() == Some(url.trim_end_matches('/')) {
            return Err("An escrow vendor must be another node.".into());
        }
        Ok(url.clone())
    }

    /// Refuse escrow before any letter is sealed when it cannot run.
    pub(crate) fn escrow_ready(&self, seller: &str) -> Result<(), OrderError> {
        if self.network != NetworkMode::Testnet4 {
            return Err(OrderError::Invalid(ESCROW_MAINNET_REFUSAL));
        }
        if self.own_url().is_none() {
            return Err(OrderError::Invalid(
                "This node has no escrow mailbox URL, so a vendor cannot reply.",
            ));
        }
        if self.vendor_url(seller).is_err() {
            return Err(OrderError::Invalid(
                "No node for this seller is set up to take escrow.",
            ));
        }
        // The request carries a per-order key from the spend tier.
        self.require_unlocked()
    }

    /// The escrow record for a new order: a fresh buyer key and payout. The
    /// address comes later, from the vendor's terms.
    pub(crate) fn escrow_request(&self, total: u64, seller: &str, listing_id: Option<[u8; 16]>) -> Result<Value, OrderError> {
        self.escrow_ready(seller)?;
        let record = moderator_record(self.network).map_err(OrderError::Failed)?;
        let spend_fee = crate::escrow_fee::presigned_fee(self.chain.fee_rate().ok(), &record);
        let funding = total
            .checked_add(spend_fee)
            .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
        let (index, payout) = {
            let mut wallet = self.buyer_wallet();
            let index = wallet
                .next_escrow_index()
                .map_err(|err| OrderError::Failed(err.to_string()))?;
            (
                index,
                wallet
                    .fresh_address()
                    .map_err(|err| OrderError::Failed(err.to_string()))?,
            )
        };
        let mut id = [0u8; 16];
        rand::RngCore::fill_bytes(
            &mut <rand::rngs::StdRng as rand::SeedableRng>::from_os_rng(),
            &mut id,
        );
        let mut value = json!({
            "network": self.network.as_str(),
            "demo": true,
            "unsafe": true,
            "warning": ESCROW_DEMO_WARNING,
            "id": id_hex(&id),
            "stage": "requested",
            "fundingSats": funding,
            "spendFeeSats": spend_fee,
            "moderatorFeeSats": MODERATOR_FEE_SATS,
            "buyerKeyIndex": index,
            "buyerPayout": payout.to_string(),
            "signaturesRequired": 2,
            "refundHeld": false,
            "safeToSend": false,
            "fundWith": "node",
        });
        if let Some(id) = listing_id {
            value["listingId"] = json!(hex(&id));
        }
        Ok(value)
    }

    /// Send the request for a stored order. When it is refused or only
    /// queued, the reason is kept on the order and returned.
    pub(crate) fn send_escrow_request(&self, order_id: &str) -> Result<(), String> {
        let error = match self.try_send_request(order_id) {
            Ok(Sent::Delivered) => return Ok(()),
            Ok(Sent::Queued(err)) => format!("The vendor's node could not be reached yet ({err}). This node keeps trying."),
            Err(err) => err,
        };
        let _ = self.update_escrow(order_id, None, json!({ "error": error }));
        Err(error)
    }

    fn try_send_request(&self, order_id: &str) -> Result<Sent, String> {
        let order = self
            .orders()
            .get(order_id)
            .cloned()
            .ok_or("that order is not on this node")?;
        let escrow = order.escrow.as_ref().ok_or("that order has no escrow")?;
        let id = escrow_id(escrow)?;
        let buyer = {
            let wallet = self.buyer_wallet();
            let secret = self.escrow_secret(&wallet, key_index(escrow)?)?;
            PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize()
        };
        let payout = address_for(text(escrow, "buyerPayout")?, self.network)?;
        let mut fields = vec![
            buyer.to_vec(),
            payout.script_pubkey().to_bytes(),
            self.buyer_mail_secret().public_key().as_bytes().to_vec(),
            self.escrow_reply_url(&id)?.as_bytes().to_vec(),
            number(escrow, "fundingSats")?.to_le_bytes().to_vec(),
            number(escrow, "spendFeeSats")?.to_le_bytes().to_vec(),
        ];
        if let Some(listing) = listing_bytes(escrow.get("listingId"))? {
            fields.push(listing.to_vec());
        }
        let mail = Mail::new(Kind::Request, id, fields);
        self.send_mail(
            &self.vendor_url(&order.seller)?,
            &self.seller_mail_key(&order.seller)?,
            &mail,
        )
    }

    fn escrow_order(&self, id: &EscrowId) -> Result<BuyerOrder, String> {
        let wanted = id_hex(id);
        self.orders()
            .on_network()
            .find(|order| {
                order
                    .escrow
                    .as_ref()
                    .and_then(|escrow| escrow.get("id"))
                    .and_then(Value::as_str)
                    == Some(&wanted)
            })
            .cloned()
            .ok_or_else(|| "no order for that escrow".into())
    }

    /// Merge `fields` into the order's escrow record, and set the status.
    fn update_escrow(
        &self,
        order_id: &str,
        status: Option<&str>,
        fields: Value,
    ) -> Result<(), String> {
        let Value::Object(fields) = fields else {
            return Err("escrow fields must be an object".into());
        };
        self.orders()
            .update(order_id, |order| {
                let escrow = order
                    .escrow
                    .get_or_insert_with(|| Value::Object(Map::new()));
                if let Value::Object(map) = escrow {
                    map.extend(fields);
                }
                if let Some(status) = status {
                    order.status = status.to_string();
                }
            })
            .map(|_| ())
    }

    pub(crate) fn buyer_mail(&self, mail: Mail) -> Result<(), String> {
        let order = self.escrow_order(&mail.escrow)?;
        match mail.kind {
            Kind::Offer => self.buyer_offer(&order, &mail),
            Kind::Reply => self.buyer_reply(&order, &mail),
            Kind::ReleaseNonce => self.buyer_release_nonce(&order, &mail),
            Kind::Ruling => self.buyer_ruling(&order, &mail),
            other => Err(format!("a buyer does not take {other:?}")),
        }
    }

    /// Check the vendor's terms against what this node asked for, and countersign.
    fn buyer_offer(&self, order: &BuyerOrder, mail: &Mail) -> Result<(), String> {
        let escrow = order.escrow.as_ref().ok_or("that order has no escrow")?;
        if text(escrow, "stage")? != "requested" {
            // A repeated offer after this node already countersigned.
            return Ok(());
        }
        let fields = mail.as_fields();
        if fields.len() != 3 && fields.len() != 4 {
            return Err("an Offer message carries 3 fields".into());
        }
        let terms = EscrowTerms::decode(&fields[0]).map_err(failed)?;
        let vendor_signature: [u8; 64] = fields[1]
            .as_slice()
            .try_into()
            .map_err(|_| "bad terms signature")?;
        let store_signature =
            bitcoin::secp256k1::schnorr::Signature::from_slice(&fields[2]).map_err(failed)?;

        let secp = Secp256k1::new();
        let store_key = self.seller_store_key(&order.seller)?;
        let listing_id = listing_bytes(escrow.get("listingId"))?;
        let digest = match (listing_id, fields.get(3)) {
            (Some(id), Some(sent)) if sent.as_slice() == id.as_slice() => offer_digest_with_listing(&mail.escrow, &terms, &id),
            (None, None) => offer_digest(&mail.escrow, &terms),
            _ => return Err("the offer's listing is not the one this order asked for".into()),
        };
        secp.verify_schnorr(&store_signature, &digest, &store_key)
            .map_err(|_| "the offer is not signed by the seller's store key")?;

        let secret = {
            let wallet = self.buyer_wallet();
            self.escrow_secret(&wallet, key_index(escrow)?)?
        };
        let payout = address_for(text(escrow, "buyerPayout")?, self.network)?;
        let ctx = self.escrow_context()?;
        if terms.buyer != PublicKey::from_secret_key(&secp, &secret).serialize()
            || terms.buyer_payout != payout.script_pubkey()
            || terms.funding_value != Amount::from_sat(number(escrow, "fundingSats")?)
            || terms.network_fee != Amount::from_sat(number(escrow, "spendFeeSats")?)
            || terms.moderator.fee != Amount::from_sat(number(escrow, "moderatorFeeSats")?)
            || terms.moderator != ctx.moderator
        {
            return Err("the vendor's terms are not the ones this node asked for".into());
        }
        let moderator: [u8; 32] = terms.moderator.signing_key[1..].try_into().map_err(failed)?;
        let store = self.seller_store(&order.seller)?;
        if !store.details.as_ref().is_some_and(|details| details.moderators.contains(&moderator)) {
            return Err("the seller's signed store record does not accept this moderator".into());
        }
        terms.validate(&ctx.policy).map_err(failed)?;
        terms
            .validate_at_tip(&ctx.policy, ctx.tip)
            .map_err(failed)?;
        let buyer_signature = terms.sign(&secret.secret_bytes()).map_err(failed)?;
        let signed =
            SignedTerms::new(terms.clone(), buyer_signature, vendor_signature).map_err(failed)?;
        let funding = terms.funding_output().map_err(failed)?;
        let address =
            Address::from_script(&funding.script_pubkey, self.network.bitcoin()).map_err(failed)?;
        let vendor_payout =
            Address::from_script(&terms.vendor_payout, self.network.bitcoin()).map_err(failed)?;
        self.update_escrow(
            &order.id,
            Some(AWAITING_FUNDING),
            json!({
                "stage": "offered",
                "error": null,
                "terms": hex(&signed.encode()),
                "address": address.to_string(),
                "vendorPayout": vendor_payout.to_string(),
                "moderatorFeeSats": terms.moderator.fee.to_sat(),
                "disputeDeadline": terms.dispute_deadline,
                "vendorClaimHeight": terms.vendor_claim_height,
                "refundHeight": terms.refund_height,
                "storeSignature": hex(&fields[2]),
                "storeKey": hex(&store_key.serialize()),
            }),
        )
    }

    /// `POST /orders/{id}/pay` for escrow: round 1 on an unsigned funding PSBT
    /// that pays exactly what the spend ticket allows.
    pub(crate) fn open_escrow(&self, order: BuyerOrder, allowed: &SpendTerms, link_coins: bool) -> Result<Value, PayError> {
        let escrow = order
            .escrow
            .clone()
            .ok_or_else(|| PayError::Invalid("This order has no escrow terms.".into()))?;
        let stage = text(&escrow, "stage").map_err(PayError::Invalid)?;
        if stage == "signed" {
            return self.rebroadcast_funding(&order, &escrow, allowed);
        }
        if stage != "offered" {
            return Err(PayError::Conflict(format!("This escrow is {stage}.")));
        }
        let id = escrow_id(&escrow).map_err(PayError::Invalid)?;
        let lock = self.escrow.locks.get(&id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let signed = unhex(text(&escrow, "terms").map_err(PayError::Invalid)?)
            .and_then(|bytes| SignedTerms::decode(&bytes).map_err(failed))
            .map_err(PayError::Invalid)?;
        let address = self.order_address(Some(&escrow), "address")?;
        let funding = signed.terms().funding_value.to_sat();
        let vendor = self.vendor_url(&order.seller).map_err(PayError::Invalid)?;
        let vendor_key = self.seller_mail_key(&order.seller).map_err(PayError::Invalid)?;

        self.sync_buyer_wallet().map_err(PayError::Chain)?;
        let fee_rate = self.pay_fee_rate()?;
        let ctx = self.escrow_context().map_err(PayError::Chain)?;
        check_terms(allowed, &address, funding)?;
        let psbt = self.build_payment(&address, funding, fee_rate, true, link_coins)?;
        check_fee(allowed, psbt_fee(&psbt)?)?;
        let secret = {
            let wallet = self.buyer_wallet();
            self.escrow_secret(&wallet, key_index(&escrow).map_err(PayError::Invalid)?)
                .map_err(PayError::Failed)?
                .secret_bytes()
        };
        let inputs: Vec<_> = psbt.unsigned_tx.input.iter().map(|input| input.previous_output).collect();
        let stored_psbt = hex(&psbt.serialize());
        let (waiting, open) =
            BuyerSession::start(signed.clone(), &secret, &ctx, FundingPlan::Psbt(psbt))
                .map_err(|err| PayError::Failed(err.to_string()))?;
        let outpoint = waiting.funding_outpoint();
        self.escrow
            .coins
            .lock(&order.id, &inputs, Some(now_secs() + OPEN_LOCK_TTL.as_secs()))
            .map_err(PayError::Failed)?;
        self.buyer_desk().waiting.insert(id, waiting);
        // Attempts only grow, so a late reply to an aborted open never fits a new one.
        let attempt = escrow.get("openAttempt").and_then(Value::as_u64).unwrap_or(0) as u32 + 1;
        self.update_escrow(
            &order.id,
            None,
            json!({ "stage": "opened", "outpoint": outpoint.to_string(), "psbt": stored_psbt, "openAttempt": attempt }),
        )
        .map_err(PayError::Failed)?;
        let mail = Mail::new(Kind::Open, id, vec![signed.encode(), open.to_bytes(), attempt_bytes(attempt)]);
        if let Err(err) = self.send_mail(&vendor, &vendor_key, &mail) {
            self.abort_open(&order.id, &id, &err);
            return Err(PayError::Chain(format!(
                "The vendor's node refused the escrow: {err}"
            )));
        }
        Ok(json!({
            "orderId": order.id,
            "network": self.network.as_str(),
            "txid": outpoint.txid.to_string(),
            "outpoint": outpoint.to_string(),
            "amountSats": funding,
            "feeRateSatVb": fee_rate.to_sat_per_kwu() as f64 / 250.0,
            "stage": "opened",
            "payjoin": false,
        }))
    }

    fn rebroadcast_funding(&self, order: &BuyerOrder, escrow: &Value, allowed: &SpendTerms) -> Result<Value, PayError> {
        let tx: Transaction =
            deserialize_hex(text(escrow, "fundingTx").map_err(PayError::Invalid)?)
                .map_err(|err| PayError::Failed(err.to_string()))?;
        if !tx
            .output
            .iter()
            .any(|out| out.script_pubkey == allowed.recipient && out.value.to_sat() == allowed.amount_sats)
        {
            return Err(PayError::Conflict(TERMS_CHANGED.into()));
        }
        let fee = self.buyer_wallet().wallet.calculate_fee(&tx).map(|fee| fee.to_sat()).unwrap_or(u64::MAX);
        check_fee(allowed, fee)?;
        self.broadcast_funding(order, &tx)?;
        Ok(json!({
            "orderId": order.id,
            "network": self.network.as_str(),
            "txid": tx.compute_txid().to_string(),
            "stage": "funded",
            "refundHeld": true,
        }))
    }

    fn broadcast_funding(&self, order: &BuyerOrder, tx: &Transaction) -> Result<(), PayError> {
        self.chain.broadcast(tx).map_err(PayError::Chain)?;
        let txid = tx.compute_txid().to_string();
        self.after_broadcast(&order.id, tx.clone(), &txid)?;
        if let Err(err) = self.escrow.coins.release(&order.id) {
            eprintln!("ob-node: coin locks for {}: {err}", order.id);
        }
        self.update_escrow(&order.id, None, json!({ "stage": "funded", "error": null }))
            .map_err(PayError::Failed)
    }

    /// Round 1 again, after a restart lost the session: fresh nonces on the
    /// stored PSBT, and the next attempt number.
    fn reopen(&self, order: &BuyerOrder, escrow: &Value, id: &EscrowId) -> Result<(), String> {
        let signed = SignedTerms::decode(&unhex(text(escrow, "terms")?)?).map_err(failed)?;
        let psbt = Psbt::deserialize(&unhex(text(escrow, "psbt")?)?).map_err(failed)?;
        let attempt = u32::try_from(number(escrow, "openAttempt")?).map_err(failed)? + 1;
        let secret = {
            let wallet = self.buyer_wallet();
            self.escrow_secret(&wallet, key_index(escrow)?)?.secret_bytes()
        };
        let ctx = self.escrow_context()?;
        let (waiting, open) = BuyerSession::start(signed.clone(), &secret, &ctx, FundingPlan::Psbt(psbt)).map_err(failed)?;
        if waiting.funding_outpoint().to_string() != text(escrow, "outpoint")? {
            return Err("the stored funding PSBT no longer matches the escrow".into());
        }
        self.buyer_desk().waiting.insert(*id, waiting);
        self.update_escrow(&order.id, None, json!({ "openAttempt": attempt }))?;
        let mail = Mail::new(Kind::Open, *id, vec![signed.encode(), open.to_bytes(), attempt_bytes(attempt)]);
        self.send_mail(&self.vendor_url(&order.seller)?, &self.seller_mail_key(&order.seller)?, &mail)
            .map(|_| ())
    }

    /// Round 3: verify the vendor, store the refund, sign, then broadcast.
    fn buyer_reply(&self, order: &BuyerOrder, mail: &Mail) -> Result<(), String> {
        let escrow = order.escrow.as_ref().ok_or("that order has no escrow")?;
        let fields = mail.fields(2)?;
        let attempt = read_attempt(&fields[1])?;
        match text(escrow, "stage")? {
            "opened" => {}
            // A repeated reply: send the stored signatures again.
            "signed" | "funded" => return self.resend_signatures(order, escrow, &mail.escrow),
            stage => return Err(format!("this escrow is {stage}")),
        }
        if u64::from(attempt) != number(escrow, "openAttempt")? {
            return Err(format!("a reply to open attempt {attempt}, not the current one"));
        }
        let reply = VendorReply::from_bytes(&fields[0]).map_err(failed)?;
        let Some(waiting) = self.buyer_desk().waiting.remove(&mail.escrow) else {
            return self.reopen(order, escrow, &mail.escrow);
        };
        let (buyer, signatures) = match waiting.receive(&reply) {
            Ok(done) => done,
            Err(err) => {
                self.abort_open(&order.id, &mail.escrow, &err.to_string());
                let _ = self.update_escrow(
                    &order.id,
                    None,
                    json!({ "stage": "failed", "error": err.to_string() }),
                );
                return Err(err.to_string());
            }
        };
        let mut psbt = buyer
            .funding_psbt()
            .cloned()
            .ok_or("the escrow kept no funding PSBT")?;
        let tx = {
            let wallet = self.buyer_wallet();
            let seed = self.spend_seed(&wallet).map_err(|err| format!("{err:?}"))?;
            if !wallet.sign(&mut psbt, &seed).map_err(failed)? {
                return Err("the funding could not be fully signed".into());
            }
            psbt.extract_tx().map_err(failed)?
        };
        if tx.compute_txid() != buyer.funding_outpoint().txid {
            return Err("signing changed the funding txid".into());
        }
        let refund = buyer.refund();
        self.save_buyer_snapshot(&mail.escrow, &buyer)?;
        self.escrow.coins.hold(&order.id)?;
        self.update_escrow(
            &order.id,
            None,
            json!({
                "stage": "signed",
                "refund": hex(&refund.to_bytes()),
                "refundTxid": refund.transaction().compute_txid().to_string(),
                "fundingTx": serialize_hex(&tx),
                "signatures": hex(&signatures.to_bytes()),
                "refundHeld": true,
                "safeToSend": true,
                "psbt": null,
            }),
        )?;
        self.buyer_desk().escrows.insert(mail.escrow, buyer);
        let reply = Mail::new(Kind::Signatures, mail.escrow, vec![signatures.to_bytes()]);
        self.send_mail(
            &self.vendor_url(&order.seller)?,
            &self.seller_mail_key(&order.seller)?,
            &reply,
        )?;
        self.broadcast_funding(order, &tx).map_err(|err| match err {
            PayError::Chain(message)
            | PayError::Failed(message)
            | PayError::Invalid(message)
            | PayError::Conflict(message) => message,
            PayError::Locked(_) => crate::lock_http::LOCKED_MESSAGE.into(),
            PayError::NotFound => "that order is gone".into(),
        })
    }

    fn resend_signatures(&self, order: &BuyerOrder, escrow: &Value, id: &EscrowId) -> Result<(), String> {
        let signatures = unhex(text(escrow, "signatures")?)?;
        let mail = Mail::new(Kind::Signatures, *id, vec![signatures]);
        self.send_mail(&self.vendor_url(&order.seller)?, &self.seller_mail_key(&order.seller)?, &mail)
            .map(|_| ())
    }

    /// Keep the vendor's release nonce. A nonce that arrives after this node
    /// signed the release means the vendor lost the old one (it restarted):
    /// sign again with it.
    fn buyer_release_nonce(&self, order: &BuyerOrder, mail: &Mail) -> Result<(), String> {
        let nonce = PubNonce::from_bytes(&mail.fields(1)?[0]).map_err(failed)?;
        let escrow = order.escrow.as_ref().ok_or("that order has no escrow")?;
        self.update_escrow(
            &order.id,
            None,
            json!({ "releaseReady": true, "releaseNonce": hex(&nonce.serialize()) }),
        )?;
        if text(escrow, "stage")? != "released" {
            return Ok(());
        }
        let signature = self.with_buyer_escrow(order, &mail.escrow, |escrow| escrow.sign_release(&nonce).map_err(failed))?;
        let release = Mail::new(Kind::Release, mail.escrow, vec![signature.to_bytes()]);
        self.send_mail(&self.vendor_url(&order.seller)?, &self.seller_mail_key(&order.seller)?, &release)?;
        self.update_escrow(&order.id, None, json!({ "releaseReady": false }))
    }

    /// The order and escrow id of an escrow in one of `stages`.
    fn funded_escrow(&self, id: &str, stages: &[&str]) -> Result<(BuyerOrder, EscrowId), PayError> {
        if self.network != NetworkMode::Testnet4 {
            return Err(PayError::Invalid(ESCROW_MAINNET_REFUSAL.into()));
        }
        let order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        let escrow = order
            .escrow
            .as_ref()
            .ok_or_else(|| PayError::Invalid("This order has no escrow.".into()))?;
        let stage = text(escrow, "stage").map_err(PayError::Invalid)?;
        if ![FUNDED, DISPUTED].contains(&order.status.as_str()) || !stages.contains(&stage) {
            return Err(PayError::Conflict(format!(
                "This escrow is {}.",
                order.status
            )));
        }
        let escrow_id = escrow_id(escrow).map_err(PayError::Invalid)?;
        Ok((order, escrow_id))
    }

    /// Sign the happy-path release and send it to the vendor.
    pub fn release_escrow(&self, id: &str) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        let (order, escrow_id) = self.funded_escrow(id, &["funded"])?;
        let lock = self.escrow.locks.get(&escrow_id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let nonce = order
            .escrow
            .as_ref()
            .and_then(|escrow| escrow.get("releaseNonce"))
            .and_then(Value::as_str)
            .and_then(|text| unhex(text).ok())
            .and_then(|bytes| PubNonce::from_bytes(&bytes).ok())
            .ok_or_else(|| PayError::Conflict("The vendor has not sent a release nonce yet.".into()))?;
        let signature = self
            .with_buyer_escrow(&order, &escrow_id, |escrow| escrow.sign_release(&nonce).map_err(failed))
            .map_err(PayError::Failed)?;
        let vendor = self.vendor_url(&order.seller).map_err(PayError::Invalid)?;
        let key = self.seller_mail_key(&order.seller).map_err(PayError::Invalid)?;
        self.update_escrow(
            id,
            Some(RELEASED),
            json!({ "stage": "released", "releaseReady": false }),
        )
        .map_err(PayError::Failed)?;
        self.send_mail(
            &vendor,
            &key,
            &Mail::new(Kind::Release, escrow_id, vec![signature.to_bytes()]),
        )
        .map_err(|err| PayError::Chain(format!("The vendor's node refused the release: {err}")))?;
        self.orders().get_value(id).ok_or(PayError::NotFound)
    }

    /// Send a dispute letter for the buyer-wins branch to the moderator.
    /// Sending again while disputed resends a fresh letter (for example after
    /// the moderator's rate limit refused the first).
    pub fn dispute_escrow(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        let memo = match body.as_object() {
            Some(map) if map.keys().all(|key| key == "memo") => {
                map.get("memo").and_then(Value::as_str).unwrap_or("")
            }
            _ => return Err(PayError::Invalid("Send a JSON object with a memo.".into())),
        };
        if memo.trim().is_empty() || memo.len() > MAX_MEMO {
            return Err(PayError::Invalid(format!(
                "Describe the problem in 1 to {MAX_MEMO} characters."
            )));
        }
        self.lock.spend_key()?;
        let moderator = self.escrow.config.moderator_url.clone().ok_or_else(|| {
            PayError::Invalid("This node has no moderator to send a dispute to.".into())
        })?;
        let (order, escrow_id) = self.funded_escrow(id, &["funded", "disputed"])?;
        let reply = self.escrow_reply_url(&escrow_id).map_err(PayError::Invalid)?.to_string();
        let lock = self.escrow.locks.get(&escrow_id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let letter = self
            .with_buyer_escrow(&order, &escrow_id, |escrow| escrow.dispute_letter(memo.as_bytes()).map_err(failed))
            .map_err(PayError::Failed)?;
        let was = order.escrow.as_ref().and_then(|escrow| escrow.get("stage")).cloned();
        self.update_escrow(id, Some(DISPUTED), json!({ "stage": "disputed" }))
            .map_err(PayError::Failed)?;
        let mut fields = vec![
            letter,
            self.buyer_mail_secret().public_key().as_bytes().to_vec(),
            reply.into_bytes(),
        ];
        let cited = (
            listing_bytes(order.escrow.as_ref().and_then(|escrow| escrow.get("listingId"))).map_err(PayError::Invalid)?,
            bytes_of(&order, "storeKey", 32).map_err(PayError::Invalid)?,
            bytes_of(&order, "storeSignature", 64).map_err(PayError::Invalid)?,
        );
        if let (Some(listing), Some(store_key), Some(store_signature)) = cited {
            fields.push(listing.to_vec());
            fields.push(store_key);
            fields.push(store_signature);
        }
        let mail = Mail::new(Kind::Dispute, escrow_id, fields);
        if let Err(err) = self.send_mail(&moderator, &moderator_mail_key(), &mail) {
            if was.as_ref().and_then(Value::as_str) == Some("funded") {
                let _ = self.update_escrow(&order.id, Some(FUNDED), json!({ "stage": "funded" }));
            }
            return Err(PayError::Chain(format!(
                "The moderator's node refused the dispute: {err}"
            )));
        }
        self.orders().get_value(id).ok_or(PayError::NotFound)
    }

    /// Finish the buyer-wins outcome with the moderator's ruling and broadcast it.
    ///
    /// The outcome is stored before the snapshot moves to `Disputed`, so a
    /// crash in between replays to a broadcast of the stored outcome.
    fn buyer_ruling(&self, order: &BuyerOrder, mail: &Mail) -> Result<(), String> {
        let escrow = order.escrow.as_ref().ok_or("that order has no escrow")?;
        match text(escrow, "stage")? {
            "disputed" => {}
            "settled" => return Ok(()),
            stage => return Err(format!("this escrow is {stage}")),
        }
        let tx = match escrow.get("settleTx").and_then(Value::as_str) {
            Some(stored) => deserialize_hex::<Transaction>(stored).map_err(failed)?,
            None => {
                let ruling = ModeratorRuling::from_bytes(&mail.fields(1)?[0]).map_err(failed)?;
                self.settle_buyer(order, &mail.escrow, &ruling)?
            }
        };
        let txid = tx.compute_txid().to_string();
        if let Err(err) = self.chain.broadcast(&tx) {
            let _ = self.update_escrow(&order.id, None, json!({ "error": format!("The outcome {txid} is signed but not broadcast: {err}") }));
            return Err(err);
        }
        self.update_escrow(
            &order.id,
            Some(SETTLED),
            json!({ "stage": "settled", "settleTxid": txid, "error": null }),
        )
    }
}

fn listing_bytes(value: Option<&Value>) -> Result<Option<[u8; 16]>, String> {
    let Some(text) = value.and_then(Value::as_str) else {
        return Ok(None);
    };
    let bytes = unhex(text)?;
    bytes.as_slice().try_into().map(Some).map_err(|_| "bad listing id".into())
}

fn bytes_of(order: &BuyerOrder, key: &str, len: usize) -> Result<Option<Vec<u8>>, String> {
    let Some(text) = order.escrow.as_ref().and_then(|escrow| escrow.get(key)).and_then(Value::as_str) else {
        return Ok(None);
    };
    let bytes = unhex(text)?;
    if bytes.len() != len {
        return Err(format!("bad {key}"));
    }
    Ok(Some(bytes))
}
