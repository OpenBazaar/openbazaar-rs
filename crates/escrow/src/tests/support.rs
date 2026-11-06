use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use secp256k1::{Secp256k1, SecretKey};

use crate::{
    order_public_key, order_secret_key, BuyerEscrow, BuyerOpen, BuyerSession, BuyerSignatures,
    Context, Error, EscrowTerms, FundingPlan, InMemoryRulingStore, Moderator, ModeratorKeys,
    ModeratorRecord, ModeratorRuling, Policy, SignedTerms, VendorEscrow, VendorReply,
    VendorSession,
};

pub const TIP: u32 = 900_000;
pub const DEADLINE: u32 = TIP + 2_016;
pub const CLAIM: u32 = DEADLINE + 144;
pub const REFUND: u32 = CLAIM + 144;
pub const FUNDING: Amount = Amount::from_sat(100_000);
pub const NETWORK_FEE: Amount = Amount::from_sat(1_000);
pub const MODERATOR_FEE: Amount = Amount::from_sat(2_000);

pub const BUYER_LONG_TERM: [u8; 32] = [0x11; 32];
pub const VENDOR_LONG_TERM: [u8; 32] = [0x22; 32];
pub const MODERATOR_SEED: [u8; 32] = [0x33; 32];
pub const OTHER_MODERATOR_SEED: [u8; 32] = [0x44; 32];
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

pub fn moderator_keys() -> ModeratorKeys {
    ModeratorKeys::from_seed(&MODERATOR_SEED).unwrap()
}

/// The record the test moderator publishes.
pub fn record() -> ModeratorRecord {
    moderator_keys().record(p2tr(0xb3), MODERATOR_FEE, Policy::default())
}

pub fn other_record() -> ModeratorRecord {
    ModeratorKeys::from_seed(&OTHER_MODERATOR_SEED)
        .unwrap()
        .record(p2tr(0xb4), MODERATOR_FEE, Policy::default())
}

/// What both parties know at funding.
pub fn ctx() -> Context {
    Context {
        tip: TIP,
        moderator: record(),
        policy: Policy::default(),
    }
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
        moderator: record(),
        funding_value: FUNDING,
        network_fee: NETWORK_FEE,
        dispute_deadline: DEADLINE,
        vendor_claim_height: CLAIM,
        refund_height: REFUND,
        buyer_payout: p2tr(0xb1),
        vendor_payout: p2tr(0xb2),
    };
    Order {
        terms,
        buyer_secret,
        vendor_secret,
    }
}

pub fn sign(order: &Order) -> SignedTerms {
    sign_terms(order, order.terms.clone())
}

/// Sign edited terms with the order's two keys.
pub fn sign_terms(order: &Order, terms: EscrowTerms) -> SignedTerms {
    let buyer = terms.sign(&order.buyer_secret).unwrap();
    let vendor = terms.sign(&order.vendor_secret).unwrap();
    SignedTerms::new(terms, buyer, vendor).unwrap()
}

pub fn outpoint() -> OutPoint {
    OutPoint {
        txid: Txid::from_byte_array([0xab; 32]),
        vout: 1,
    }
}

/// Run all three rounds. Each side only sees the other's serialized messages.
pub fn run_funding(order: &Order, plan: FundingPlan) -> Result<(BuyerEscrow, VendorEscrow), Error> {
    run_funding_signed(order, sign(order), plan)
}

pub fn run_funding_signed(
    order: &Order,
    signed: SignedTerms,
    plan: FundingPlan,
) -> Result<(BuyerEscrow, VendorEscrow), Error> {
    let ctx = ctx();
    let (buyer, open) = BuyerSession::start(signed.clone(), &order.buyer_secret, &ctx, plan)?;
    let open = BuyerOpen::from_bytes(&open.to_bytes())?;

    let (vendor, reply) = VendorSession::accept(signed, &order.vendor_secret, &ctx, &open)?;
    let reply = VendorReply::from_bytes(&reply.to_bytes())?;

    let (buyer, signatures) = buyer.receive(&reply)?;
    let signatures = BuyerSignatures::from_bytes(&signatures.to_bytes())?;

    let vendor = vendor.receive(&signatures)?;
    Ok((buyer, vendor))
}

/// A funded pair, built once per test binary. Each test gets its own copy.
pub fn funded() -> (BuyerEscrow, VendorEscrow) {
    static FUNDED: OnceLock<(BuyerEscrow, VendorEscrow)> = OnceLock::new();
    let (buyer, vendor) = FUNDED.get_or_init(|| {
        run_funding(&order(ORDER_SECRET), FundingPlan::Outpoint(outpoint())).unwrap()
    });
    (buyer.duplicate(), vendor.duplicate())
}

pub type TestModerator = Moderator<HashMap<OutPoint, TxOut>, InMemoryRulingStore>;

/// The test moderator, seeing `chain` as the confirmed funding outputs.
pub fn moderator_with_chain(chain: HashMap<OutPoint, TxOut>) -> TestModerator {
    Moderator::new(
        moderator_keys(),
        record(),
        chain,
        InMemoryRulingStore::default(),
    )
    .unwrap()
}

/// The test moderator, seeing the default order's funding output confirmed.
pub fn moderator() -> TestModerator {
    let funding = order(ORDER_SECRET).terms.funding_output().unwrap();
    moderator_with_chain(HashMap::from([(outpoint(), funding)]))
}

/// Open a letter with a fresh moderator (fresh ruling store).
pub fn rule(letter: &[u8], height: u32) -> Result<ModeratorRuling, Error> {
    moderator().open(letter, height)
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
