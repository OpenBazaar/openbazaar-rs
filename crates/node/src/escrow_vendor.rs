//! The vendor's side of a moderated escrow (testnet4 only).
//!
//! A buyer's request names the buyer's per-order key and payout. The vendor
//! picks the heights from its own chain tip, adds a fresh per-order key from
//! the seller's store wallet seed (`m/1984'/coin'/1'/index'`), signs the
//! terms, and runs round 2 and the funding finish. Its key never leaves
//! this node and it never sees the buyer's.
//!
//! Sales live in memory. The claim is kept as bytes in the sale, but the
//! live [`VendorEscrow`] (needed for the release) is lost on restart.

use std::collections::BTreeMap;

use bitcoin::hashes::{sha256, Hash, HashEngine};
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1};
use bitcoin::{Address, Amount, OutPoint, ScriptBuf, Txid};
use openbazaar_escrow::{
    BuyerOpen, BuyerSignatures, EscrowTerms, ReleaseNonce, ReleaseSignature, SignedTerms,
    VendorAwaitingSignatures, VendorEscrow, VendorSession,
};
use openbazaar_protocol::vendor_mail_key;
use serde_json::{json, Value};
use tormail::StoreKey;

use crate::escrow_mail::{check_loopback, hex, id_hex, key_bytes, EscrowId, Kind, Mail};
use crate::instructions::ESCROW_SPEND_FEE_SATS;
use crate::network::NetworkMode;
use crate::Node;

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

struct Sale {
    seller: String,
    index: u32,
    terms: EscrowTerms,
    buyer_mail: StoreKey,
    buyer_url: String,
    stage: &'static str,
    outpoint: Option<OutPoint>,
    awaiting: Option<VendorAwaitingSignatures>,
    escrow: Option<VendorEscrow>,
    nonce: Option<ReleaseNonce>,
    claim: Option<Vec<u8>>,
    release_txid: Option<Txid>,
}

#[derive(Default)]
pub(crate) struct VendorDesk {
    sales: BTreeMap<EscrowId, Sale>,
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

    pub(crate) fn vendor_mail(&self, seller: &str, mail: Mail) -> Result<(), String> {
        match mail.kind {
            Kind::Request => self.vendor_request(seller, &mail),
            Kind::Open => self.vendor_open(seller, &mail),
            Kind::Signatures => self.vendor_signatures(seller, &mail),
            Kind::Release => self.vendor_release(seller, &mail),
            other => Err(format!("a vendor does not take {other:?}")),
        }
    }

    /// Round 0: answer a request with terms signed by the vendor's per-order key.
    fn vendor_request(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.fields(5)?;
        let buyer: [u8; 33] = fields[0]
            .as_slice()
            .try_into()
            .map_err(|_| "bad buyer key")?;
        let buyer_payout = ScriptBuf::from_bytes(fields[1].clone());
        let buyer_mail = key_bytes(&fields[2])?;
        let buyer_url = String::from_utf8(fields[3].clone()).map_err(failed)?;
        check_loopback(&buyer_url)?;
        if self.own_url() == Some(buyer_url.trim_end_matches('/')) {
            return Err("this node will not sell to itself".into());
        }
        let funding: [u8; 8] = fields[4]
            .as_slice()
            .try_into()
            .map_err(|_| "bad funding amount")?;
        if self.vendor_desk().sales.contains_key(&mail.escrow) {
            return Err("that escrow already has terms".into());
        }
        let ctx = self.escrow_context()?;
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
            network_fee: Amount::from_sat(ESCROW_SPEND_FEE_SATS),
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
        let store = vendor_mail_key(seller).ok_or("that seller has no store key")?;
        let secp = Secp256k1::new();
        let store_signature = secp
            .sign_schnorr_no_aux_rand(
                &offer_digest(&mail.escrow, &terms),
                &Keypair::from_secret_key(&secp, &store),
            )
            .serialize();
        let offer = Mail::new(
            Kind::Offer,
            mail.escrow,
            vec![
                terms.encode(),
                vendor_signature.to_vec(),
                store_signature.to_vec(),
            ],
        );
        self.send_mail(&buyer_url, &buyer_mail, &offer)?;
        self.vendor_desk().sales.insert(
            mail.escrow,
            Sale {
                seller: seller.to_string(),
                index,
                terms,
                buyer_mail,
                buyer_url,
                stage: "offered",
                outpoint: None,
                awaiting: None,
                escrow: None,
                nonce: None,
                claim: None,
                release_txid: None,
            },
        );
        Ok(())
    }

    /// Round 2: check the buyer's opening and sign the vendor's half.
    fn vendor_open(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.fields(2)?;
        let signed = SignedTerms::decode(&fields[0]).map_err(failed)?;
        let open = BuyerOpen::from_bytes(&fields[1]).map_err(failed)?;
        let (index, url, to) = {
            let desk = self.vendor_desk();
            let sale = desk
                .sales
                .get(&mail.escrow)
                .ok_or("no sale for that escrow")?;
            if sale.seller != seller || sale.stage != "offered" {
                return Err(format!("that sale is {}", sale.stage));
            }
            if signed.terms() != &sale.terms {
                return Err("the buyer signed different terms".into());
            }
            (sale.index, sale.buyer_url.clone(), sale.buyer_mail)
        };
        let secret = self.vendor_secret(seller, index)?;
        let ctx = self.escrow_context()?;
        let (awaiting, reply) =
            VendorSession::accept(signed, &secret, &ctx, &open).map_err(failed)?;
        self.send_mail(
            &url,
            &to,
            &Mail::new(Kind::Reply, mail.escrow, vec![reply.to_bytes()]),
        )?;
        let mut desk = self.vendor_desk();
        let sale = desk
            .sales
            .get_mut(&mail.escrow)
            .ok_or("no sale for that escrow")?;
        sale.awaiting = Some(awaiting);
        sale.outpoint = Some(open.funding_outpoint);
        sale.stage = "replied";
        Ok(())
    }

    /// Funding finish: keep the claim, then offer a release nonce.
    fn vendor_signatures(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.fields(1)?;
        let signatures = BuyerSignatures::from_bytes(&fields[0]).map_err(failed)?;
        let (url, to, nonce) = {
            let mut desk = self.vendor_desk();
            let sale = desk
                .sales
                .get_mut(&mail.escrow)
                .ok_or("no sale for that escrow")?;
            if sale.seller != seller || sale.stage != "replied" {
                return Err(format!("that sale is {}", sale.stage));
            }
            let awaiting = sale.awaiting.take().ok_or("that sale has no open round")?;
            let escrow = match awaiting.receive(&signatures) {
                Ok(escrow) => escrow,
                Err(err) => {
                    sale.stage = "failed";
                    return Err(err.to_string());
                }
            };
            let nonce = escrow.release_nonce().map_err(failed)?;
            let public = nonce.public().serialize().to_vec();
            sale.claim = Some(escrow.claim().to_bytes());
            sale.escrow = Some(escrow);
            sale.nonce = Some(nonce);
            sale.stage = "funded";
            (sale.buyer_url.clone(), sale.buyer_mail, public)
        };
        self.send_mail(
            &url,
            &to,
            &Mail::new(Kind::ReleaseNonce, mail.escrow, vec![nonce]),
        )
    }

    /// Happy path: finish the release with the buyer's signature and broadcast it.
    fn vendor_release(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        let fields = mail.fields(1)?;
        let signature = ReleaseSignature::from_bytes(&fields[0]).map_err(failed)?;
        let tx = {
            let mut desk = self.vendor_desk();
            let sale = desk
                .sales
                .get_mut(&mail.escrow)
                .ok_or("no sale for that escrow")?;
            if sale.seller != seller || sale.stage != "funded" {
                return Err(format!("that sale is {}", sale.stage));
            }
            let nonce = sale.nonce.take().ok_or("no release nonce is open")?;
            let escrow = sale
                .escrow
                .as_mut()
                .ok_or("this node restarted since funding")?;
            escrow.complete_release(nonce, &signature).map_err(failed)?
        };
        self.chain.broadcast(&tx)?;
        let mut desk = self.vendor_desk();
        if let Some(sale) = desk.sales.get_mut(&mail.escrow) {
            sale.stage = "released";
            sale.release_txid = Some(tx.compute_txid());
        }
        Ok(())
    }
}
