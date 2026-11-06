use std::collections::HashSet;
use std::sync::OnceLock;

use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, Txid};
use secp256k1::{Secp256k1, SecretKey};

use crate::{
    order_public_key, order_secret_key, BuyerEscrow, BuyerOpen, BuyerSession, BuyerSignatures,
    Error, EscrowTerms, FundingPlan, Policy, SignedTerms, VendorEscrow, VendorReply, VendorSession,
};

pub const DEADLINE: u32 = 1_000;
pub const CLAIM: u32 = 1_144;
pub const REFUND: u32 = 1_288;
pub const FUNDING: Amount = Amount::from_sat(100_000);
pub const NETWORK_FEE: Amount = Amount::from_sat(1_000);
pub const MODERATOR_FEE: Amount = Amount::from_sat(2_000);

pub const BUYER_LONG_TERM: [u8; 32] = [0x11; 32];
pub const VENDOR_LONG_TERM: [u8; 32] = [0x22; 32];
pub const MODERATOR: [u8; 32] = [0x33; 32];
pub const OTHER_MODERATOR: [u8; 32] = [0x44; 32];
pub const ORDER_SECRET: [u8; 32] = [0x55; 32];

pub fn public(secret: &[u8; 32]) -> [u8; 33] {
    let secp = Secp256k1::signing_only();
    SecretKey::from_byte_array(*secret)
        .unwrap()
        .public_key(&secp)
        .serialize()
}

pub fn p2tr(seed: u8) -> ScriptBuf {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let key = bitcoin::secp256k1::SecretKey::from_slice(&[seed; 32]).unwrap();
    let (xonly, _) = key.x_only_public_key(&secp);
    ScriptBuf::new_p2tr(&secp, xonly, None)
}

pub fn moderator_public() -> [u8; 33] {
    public(&MODERATOR)
}

/// One order as each party sees it: the shared terms, and each side's own key.
pub struct Order {
    pub terms: EscrowTerms,
    pub buyer_secret: [u8; 32],
    pub vendor_secret: [u8; 32],
}

pub fn order(order_secret: [u8; 32]) -> Order {
    let buyer_secret = order_secret_key(&BUYER_LONG_TERM, &order_secret).unwrap();
    let vendor_secret = order_secret_key(&VENDOR_LONG_TERM, &order_secret).unwrap();
    let terms = EscrowTerms {
        buyer: order_public_key(&public(&BUYER_LONG_TERM), &order_secret).unwrap(),
        vendor: order_public_key(&public(&VENDOR_LONG_TERM), &order_secret).unwrap(),
        moderator: moderator_public(),
        funding_value: FUNDING,
        network_fee: NETWORK_FEE,
        moderator_fee: MODERATOR_FEE,
        dispute_deadline: DEADLINE,
        vendor_claim_height: CLAIM,
        refund_height: REFUND,
        buyer_payout: p2tr(0xb1),
        vendor_payout: p2tr(0xb2),
        moderator_payout: p2tr(0xb3),
    };
    Order {
        terms,
        buyer_secret,
        vendor_secret,
    }
}

pub fn sign(order: &Order) -> SignedTerms {
    let buyer = order.terms.sign(&order.buyer_secret).unwrap();
    let vendor = order.terms.sign(&order.vendor_secret).unwrap();
    SignedTerms::new(order.terms.clone(), buyer, vendor).unwrap()
}

pub fn outpoint() -> OutPoint {
    OutPoint {
        txid: Txid::from_byte_array([0xab; 32]),
        vout: 1,
    }
}

/// Run all three rounds. Each side only sees the other's serialized messages.
pub fn run_funding(order: &Order, plan: FundingPlan) -> Result<(BuyerEscrow, VendorEscrow), Error> {
    let policy = Policy::default();
    let moderator = moderator_public();

    let (buyer, open) =
        BuyerSession::start(sign(order), &order.buyer_secret, &moderator, plan, &policy)?;
    let open = BuyerOpen::from_bytes(&open.to_bytes())?;

    let (vendor, reply) = VendorSession::accept(
        sign(order),
        &order.vendor_secret,
        &moderator,
        &open,
        &policy,
    )?;
    let reply = VendorReply::from_bytes(&reply.to_bytes())?;

    let (buyer, signatures) = buyer.receive(&reply)?;
    let signatures = BuyerSignatures::from_bytes(&signatures.to_bytes())?;

    let vendor = vendor.receive(&signatures)?;
    Ok((buyer, vendor))
}

/// A funded pair, built once per test binary. Clone it per test.
pub fn funded() -> (BuyerEscrow, VendorEscrow) {
    static FUNDED: OnceLock<(BuyerEscrow, VendorEscrow)> = OnceLock::new();
    FUNDED
        .get_or_init(|| {
            run_funding(&order(ORDER_SECRET), FundingPlan::Outpoint(outpoint())).unwrap()
        })
        .clone()
}

/// Minimal chain: enforces `nLockTime` finality for the next block and single spends.
pub struct Chain {
    pub tip: u32,
    spent: HashSet<OutPoint>,
}

impl Chain {
    pub fn at(tip: u32) -> Self {
        Self {
            tip,
            spent: HashSet::new(),
        }
    }

    pub fn next_height(&self) -> u32 {
        self.tip + 1
    }

    pub fn mine(&mut self, tx: &Transaction) -> Result<(), &'static str> {
        if !crate::is_final_at_height(tx, self.next_height()) {
            return Err("non-final");
        }
        for input in &tx.input {
            if !self.spent.insert(input.previous_output) {
                return Err("double spend");
            }
        }
        self.tip += 1;
        Ok(())
    }

    pub fn advance_to(&mut self, tip: u32) {
        assert!(tip >= self.tip);
        self.tip = tip;
    }
}
