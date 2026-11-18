//! The filing fee of a v5 case, on the moderator's side.
//!
//! A reviewed letter whose terms set a non-zero filing fee gets a signed
//! request ([`openbazaar_protocol::FeeRequest`]) with a fresh output script
//! of this node's wallet and, when an existing channel has room, a Lightning
//! invoice. The fee is per escrow: whoever files first is asked first, and
//! once either party pays, every letter of that escrow is covered.
//!
//! [`Node::moderator_fee_tick`] does the rest from the mail loop: it credits
//! a payment this node sees on its own Lightning node, or a confirmed output
//! of at least the fee to the request's script in its own wallet that
//! answered this escrow's request ([`super::owed`]); it signs
//! the receipt and the public case-accepted record and sends the receipt;
//! and it asks the next filer once a request expires. Nothing the filer
//! sends is trusted as proof of payment.
//!
//! Requests, receipts, and decisions are signed with the ruling key, so they
//! wait for an unlocked wallet, as rulings do. Fee scripts come from the
//! wallet's receive chain, held back from the receive page; a script that
//! was never paid goes back to a pool after the due height, so unpaid
//! requests cannot push the wallet past its recovery gap.

use std::collections::BTreeMap;

use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::hashes::Hash;
use bitcoin::{ScriptBuf, Transaction};
use openbazaar_escrow::{fee_letter_digest, Party};
use openbazaar_protocol::{
    FeeInvoice, FeePayment, FeeReceipt, FeeRequest, SignedFeeRequest, FAVORED_BUYER, FAVORED_VENDOR,
};
use serde_json::{json, Value};

use super::case::{fixed, party_name, Case};
use super::owed::{creditable, owed, payment_id, Owed, Received};
use crate::escrow_mail::{hex, id_hex, parse_id, unhex, EscrowId, Kind, Mail};
use crate::escrow_outbox::now_secs;
use crate::escrow_store::Role;
use crate::Node;

/// How long a request takes payment, unless the due height comes first.
pub(super) const REQUEST_BLOCKS: u32 = 72;
/// No request is issued this close to the due height: the filer needs time
/// to pay and the operator time to decide.
pub(super) const REQUEST_MARGIN: u32 = 6;
/// Fee scripts nothing paid yet, held by open escrows or the pool, at
/// once. Below the wallet's recovery gap of 20, so a restore from the seed
/// still finds every later payment.
pub(super) const MAX_UNPAID_SCRIPTS: usize = 16;
const TICK_SECS: u64 = 5;
const POOL: &str = "fee-pool";

/// One request sent to a filer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Requested {
    pub filer: Party,
    pub bytes: Vec<u8>,
    pub digest: [u8; 32],
    pub payment_hash: Option<String>,
    pub valid_until: u32,
}

/// A wallet receive script one filer of one escrow is asked to pay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FeeScript {
    pub filer: Party,
    pub index: u32,
    pub script: ScriptBuf,
    /// The tip when it was given to this filer.
    pub issued: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Paid {
    pub filer: Party,
    pub request: [u8; 32],
    pub payment: FeePayment,
    pub received_sats: u64,
    pub height: u32,
}

/// The filing fee of one escrow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EscrowFee {
    pub case_id: [u8; 32],
    pub amount: u64,
    pub due: u32,
    pub scripts: Vec<FeeScript>,
    pub requests: Vec<Requested>,
    /// Filers that sent their letter again after their request expired.
    pub renew: Vec<Party>,
    pub paid: Option<Paid>,
    pub receipt: Option<Vec<u8>>,
    pub accepted: Option<Vec<u8>>,
    pub receipt_sent: bool,
    /// Past the due height: scripts nothing paid went back to the pool.
    pub released: bool,
    /// The signed recusal (`OBDD`), once the operator stepped away.
    pub recusal: Option<Vec<u8>>,
    /// Refunds this node sent from its wallet, stored before broadcast.
    pub refunds: Vec<Refund>,
}

/// A refund paid from this node's wallet to a filer's refund script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Refund {
    pub filer: Party,
    pub amount: u64,
    /// The owed payment it sends back; none for a recusal's refund.
    pub owed: Option<FeePayment>,
    pub tx: Transaction,
}

impl Refund {
    fn to_record(&self) -> Value {
        json!({
            "filer": party_byte(self.filer),
            "amountSats": self.amount,
            "owed": self.owed.as_ref().map(payment_value),
            "tx": serialize_hex(&self.tx),
        })
    }

    fn from_record(value: &Value) -> Option<Self> {
        Some(Self {
            filer: party_from(value["filer"].as_u64()?)?,
            amount: value["amountSats"].as_u64()?,
            owed: match &value["owed"] {
                Value::Null => None,
                owed => Some(payment_from(owed)?),
            },
            tx: deserialize_hex(value["tx"].as_str()?).ok()?,
        })
    }

    pub fn value(&self) -> Value {
        json!({
            "filer": party_name(self.filer),
            "amountSats": self.amount,
            "owed": self.owed.as_ref().map(payment_id),
            "txid": self.tx.compute_txid().to_string(),
        })
    }
}

fn party_byte(party: Party) -> u8 {
    match party {
        Party::Buyer => FAVORED_BUYER,
        Party::Vendor => FAVORED_VENDOR,
    }
}

fn party_from(byte: u64) -> Option<Party> {
    match u8::try_from(byte).ok()? {
        FAVORED_BUYER => Some(Party::Buyer),
        FAVORED_VENDOR => Some(Party::Vendor),
        _ => None,
    }
}

fn payment_value(payment: &FeePayment) -> Value {
    match payment {
        FeePayment::Lightning { payment_hash } => json!({ "rail": "ln", "paymentHash": hex(payment_hash) }),
        FeePayment::OnChain { txid, vout } => json!({ "rail": "onchain", "txid": hex(txid), "vout": vout }),
    }
}

fn payment_from(value: &Value) -> Option<FeePayment> {
    match value["rail"].as_str()? {
        "ln" => Some(FeePayment::Lightning {
            payment_hash: fixed(value["paymentHash"].as_str()?)?,
        }),
        "onchain" => Some(FeePayment::OnChain {
            txid: fixed(value["txid"].as_str()?)?,
            vout: u32::try_from(value["vout"].as_u64()?).ok()?,
        }),
        _ => None,
    }
}

fn height(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

impl EscrowFee {
    pub fn new(case_id: [u8; 32], amount: u64, due: u32) -> Self {
        Self {
            case_id,
            amount,
            due,
            scripts: Vec::new(),
            requests: Vec::new(),
            renew: Vec::new(),
            paid: None,
            receipt: None,
            accepted: None,
            receipt_sent: false,
            released: false,
            recusal: None,
            refunds: Vec::new(),
        }
    }

    pub fn to_record(&self) -> Value {
        json!({
            "caseId": hex(&self.case_id),
            "amountSats": self.amount,
            "dueHeight": self.due,
            "scripts": self.scripts.iter().map(|script| json!({
                "filer": party_byte(script.filer),
                "index": script.index,
                "script": hex(script.script.as_bytes()),
                "issued": script.issued,
            })).collect::<Vec<_>>(),
            "requests": self.requests.iter().map(|request| json!({
                "filer": party_byte(request.filer),
                "bytes": hex(&request.bytes),
                "digest": hex(&request.digest),
                "paymentHash": request.payment_hash,
                "validUntil": request.valid_until,
            })).collect::<Vec<_>>(),
            "renew": self.renew.iter().map(|party| party_byte(*party)).collect::<Vec<_>>(),
            "paid": self.paid.map(|paid| json!({
                "filer": party_byte(paid.filer),
                "request": hex(&paid.request),
                "payment": payment_value(&paid.payment),
                "receivedSats": paid.received_sats,
                "height": paid.height,
            })),
            "receipt": self.receipt.as_deref().map(hex),
            "accepted": self.accepted.as_deref().map(hex),
            "receiptSent": self.receipt_sent,
            "released": self.released,
            "recusal": self.recusal.as_deref().map(hex),
            "refunds": self.refunds.iter().map(Refund::to_record).collect::<Vec<_>>(),
        })
    }

    pub fn from_record(value: &Value) -> Option<Self> {
        let scripts = value["scripts"].as_array()?.iter().map(|script| {
            Some(FeeScript {
                filer: party_from(script["filer"].as_u64()?)?,
                index: height(&script["index"])?,
                script: ScriptBuf::from_bytes(unhex(script["script"].as_str()?).ok()?),
                issued: height(&script["issued"]).unwrap_or(0),
            })
        });
        let requests = value["requests"].as_array()?.iter().map(|request| {
            Some(Requested {
                filer: party_from(request["filer"].as_u64()?)?,
                bytes: unhex(request["bytes"].as_str()?).ok()?,
                digest: fixed(request["digest"].as_str()?)?,
                payment_hash: request["paymentHash"].as_str().map(str::to_string),
                valid_until: height(&request["validUntil"])?,
            })
        });
        let paid = match &value["paid"] {
            Value::Null => None,
            paid => Some(Paid {
                filer: party_from(paid["filer"].as_u64()?)?,
                request: fixed(paid["request"].as_str()?)?,
                payment: payment_from(&paid["payment"])?,
                received_sats: paid["receivedSats"].as_u64()?,
                height: height(&paid["height"])?,
            }),
        };
        let optional = |key: &str| match value[key].as_str() {
            Some(text) => unhex(text).ok().map(Some),
            None => Some(None),
        };
        Some(Self {
            case_id: fixed(value["caseId"].as_str()?)?,
            amount: value["amountSats"].as_u64()?,
            due: height(&value["dueHeight"])?,
            scripts: scripts.collect::<Option<_>>()?,
            requests: requests.collect::<Option<_>>()?,
            renew: value["renew"]
                .as_array()?
                .iter()
                .map(|party| party_from(party.as_u64()?))
                .collect::<Option<_>>()?,
            paid,
            receipt: optional("receipt")?,
            accepted: optional("accepted")?,
            receipt_sent: value["receiptSent"].as_bool()?,
            released: value["released"].as_bool()?,
            recusal: optional("recusal")?,
            refunds: match value["refunds"].as_array() {
                Some(list) => list.iter().map(Refund::from_record).collect::<Option<_>>()?,
                None => Vec::new(),
            },
        })
    }

    /// The request that still takes payment at `tip`, if any.
    pub fn live(&self, tip: u32) -> Option<&Requested> {
        self.requests.last().filter(|request| request.valid_until >= tip)
    }

    /// The desk's word for where this fee stands.
    pub fn state(&self, tip: Option<u32>) -> &'static str {
        match (&self.paid, tip.and_then(|tip| self.live(tip))) {
            (Some(_), _) => "paid",
            (None, Some(_)) => "requested",
            (None, None) if self.requests.is_empty() => "awaiting-request",
            (None, None) => "expired",
        }
    }
}

pub(super) fn fee_name(id: &EscrowId) -> String {
    format!("{}-fee", id_hex(id))
}

/// Parse a stored fee record name back to its escrow.
pub(super) fn fee_id(name: &str) -> Option<EscrowId> {
    parse_id(name.strip_suffix("-fee")?)
}

/// The receive scripts of fee requests whose case closed unpaid.
#[derive(Default)]
pub(super) struct FeePool {
    pub free: Vec<(u32, ScriptBuf)>,
}

impl FeePool {
    pub fn is_pool(name: &str) -> bool {
        name == POOL
    }

    pub fn from_record(value: &Value) -> Option<Self> {
        let free = value["free"].as_array()?.iter().map(|entry| {
            Some((
                height(&entry["index"])?,
                ScriptBuf::from_bytes(unhex(entry["script"].as_str()?).ok()?),
            ))
        });
        Some(Self {
            free: free.collect::<Option<_>>()?,
        })
    }

    fn to_record(&self) -> Value {
        json!({ "free": self.free.iter().map(|(index, script)| json!({ "index": index, "script": hex(script.as_bytes()) })).collect::<Vec<_>>() })
    }
}

/// What the tick needs from one escrow's cases.
struct Filer {
    party: Party,
    received: u32,
    letter_digest: [u8; 32],
    reply_url: String,
    reply_mail: tormail::StoreKey,
    record: Option<Vec<u8>>,
    decided: bool,
}

fn filers(cases: &BTreeMap<(EscrowId, u8), Case>, id: &EscrowId) -> Vec<Filer> {
    let mut found: Vec<Filer> = cases
        .iter()
        .filter(|((case_id, _), _)| case_id == id)
        .map(|((_, branch), case)| Filer {
            party: super::case::party_of(*branch),
            received: case.received,
            letter_digest: fee_letter_digest(&case.letter),
            reply_url: case.reply_url.clone(),
            reply_mail: case.reply_mail,
            record: case.record.clone(),
            decided: case.decision.is_some() || case.ruling.is_some(),
        })
        .collect();
    found.sort_by_key(|filer| filer.received);
    found
}

/// Who is asked next: a filer never asked, earliest letter first, then one
/// that sent its letter again after its request expired.
fn next_filer<'a>(fee: &EscrowFee, filers: &'a [Filer]) -> Option<&'a Filer> {
    let asked = |party: Party| fee.requests.iter().any(|request| request.filer == party);
    let open = || filers.iter().filter(|filer| !filer.decided);
    open()
        .find(|filer| !asked(filer.party))
        .or_else(|| open().find(|filer| fee.renew.contains(&filer.party)))
}

impl super::ModeratorDesk {
    /// Whether the operator may decide a case of `id`: the escrow's fee is
    /// paid, or the case owes none.
    pub(super) fn fee_settled(&self, id: &EscrowId, case: &Case) -> bool {
        case.free() || self.fees.get(id).is_some_and(|fee| fee.paid.is_some())
    }

    /// The fee fields of one case for the desk. `decided`: some letter of
    /// the escrow has a ruling or a denial.
    pub(super) fn case_fee_value(
        &self,
        id: &EscrowId,
        case: &Case,
        tip: Option<u32>,
        decided: bool,
        received: &Received,
    ) -> Value {
        let Some(terms) = case.fee else { return Value::Null };
        if terms.amount == 0 {
            return json!({ "state": "free", "amountSats": 0, "dueHeight": terms.due, "overdue": false });
        }
        let Some(fee) = self.fees.get(id) else {
            return json!({ "state": "awaiting-request", "amountSats": terms.amount, "dueHeight": terms.due, "overdue": false });
        };
        json!({
            "state": fee.state(tip),
            "amountSats": fee.amount,
            "dueHeight": fee.due,
            "paidBy": fee.paid.map(|paid| party_name(paid.filer)),
            "rail": fee.paid.map(|paid| match paid.payment {
                FeePayment::Lightning { .. } => "ln",
                FeePayment::OnChain { .. } => "onchain",
            }),
            "paidHeight": fee.paid.map(|paid| paid.height),
            "overdue": fee.paid.is_some() && !decided && fee.recusal.is_none() && tip.is_some_and(|tip| tip > fee.due),
            "recused": fee.recusal.is_some(),
            "refunds": fee.refunds.iter().map(Refund::value).collect::<Vec<_>>(),
            "owed": owed(&self.fees, id, received).iter().map(Owed::value).collect::<Vec<_>>(),
        })
    }
}

impl Node {
    pub(super) fn save_fee(&self, id: &EscrowId, fee: &EscrowFee) -> Result<(), String> {
        self.escrow.store.save(Role::Moderator, &fee_name(id), &fee.to_record())
    }

    /// Start the fee of a reviewed v5 letter, or note that its filer asks again.
    pub(super) fn open_fee(
        &self,
        id: &EscrowId,
        filer: Party,
        case_id: [u8; 32],
        amount: u64,
        due: u32,
    ) -> Result<(), String> {
        let mut desk = self.moderator_desk();
        let tip = self.chain.tip_height().ok();
        let fee = desk
            .fees
            .entry(*id)
            .or_insert_with(|| EscrowFee::new(case_id, amount, due));
        let expired = fee
            .requests
            .iter()
            .rev()
            .find(|request| request.filer == filer)
            .is_some_and(|request| tip.is_some_and(|tip| request.valid_until < tip));
        if fee.paid.is_none() && expired && !fee.renew.contains(&filer) {
            fee.renew.push(filer);
        }
        let fee = fee.clone();
        drop(desk);
        self.save_fee(id, &fee)
    }

    /// Hold every fee script back from the wallet's receive page.
    pub(crate) fn hold_moderator_fee_addresses(&self) {
        let indices: Vec<u32> = {
            let desk = self.moderator_desk();
            let held = desk
                .fees
                .values()
                .flat_map(|fee| fee.scripts.iter().map(|script| script.index));
            held.chain(desk.pool.free.iter().map(|(index, _)| *index)).collect()
        };
        if !indices.is_empty() {
            self.buyer_wallet().hold_fee_addresses(&indices);
        }
    }

    /// Credit, sign, send, request, and release fees. Called from the mail
    /// loop; does nothing more than once every few seconds.
    pub(crate) fn moderator_fee_tick(&self) {
        {
            let mut desk = self.moderator_desk();
            let now = now_secs();
            if desk.fees.is_empty() || now < desk.fee_tick.saturating_add(TICK_SECS) {
                return;
            }
            desk.fee_tick = now;
        }
        self.moderator_fee_steps();
    }

    /// One pass of [`Self::moderator_fee_tick`] over every fee, unthrottled.
    pub(crate) fn moderator_fee_steps(&self) {
        let ids: Vec<EscrowId> = self.moderator_desk().fees.keys().copied().collect();
        let Ok(tip) = self.chain.tip_height() else { return };
        for id in ids {
            if let Err(err) = self.fee_step(&id, tip) {
                eprintln!("ob-node: dispute fee {}: {err}", id_hex(&id));
            }
        }
        let refunds: Vec<EscrowFee> = {
            let mut desk = self.moderator_desk();
            if desk.refunds_checked == tip {
                return;
            }
            desk.refunds_checked = tip;
            desk.fees.values().filter(|fee| !fee.refunds.is_empty()).cloned().collect()
        };
        for fee in &refunds {
            self.rebroadcast_refunds(fee);
        }
    }

    fn fee_step(&self, id: &EscrowId, tip: u32) -> Result<(), String> {
        let lock = self.escrow.locks.get(id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let (mut fee, filers) = {
            let desk = self.moderator_desk();
            let Some(fee) = desk.fees.get(id) else { return Ok(()) };
            (fee.clone(), filers(&desk.cases, id))
        };
        let before = fee.clone();
        if fee.paid.is_none() {
            fee.paid = self.find_payment(id, &fee, tip);
        }
        let result = self.fee_paperwork(id, &mut fee, &filers, tip);
        if tip > fee.due.saturating_add(REQUEST_MARGIN) && !fee.released {
            self.release_scripts(&mut fee);
        }
        if fee != before {
            self.save_fee(id, &fee)?;
            self.moderator_desk().fees.insert(*id, fee);
        }
        result
    }

    /// A payment this node received for one of the escrow's requests.
    fn find_payment(&self, id: &EscrowId, fee: &EscrowFee, tip: u32) -> Option<Paid> {
        let want = fee.amount;
        for request in &fee.requests {
            let Some(hash) = &request.payment_hash else { continue };
            if self
                .ln_fee_received(hash)
                .is_some_and(|msat| msat >= want.saturating_mul(1_000))
            {
                return Some(Paid {
                    filer: request.filer,
                    request: request.digest,
                    payment: FeePayment::Lightning {
                        payment_hash: fixed(hash)?,
                    },
                    received_sats: want,
                    height: tip,
                });
            }
        }
        let scripts: Vec<ScriptBuf> = fee.scripts.iter().map(|script| script.script.clone()).collect();
        let received = self.fee_received(&scripts, &[]);
        if received.onchain.is_empty() {
            return None;
        }
        let (script, output) = creditable(&self.moderator_desk().fees, id, &received)?;
        let request = fee
            .requests
            .iter()
            .rev()
            .find(|request| request.filer == script.filer)?;
        Some(Paid {
            filer: script.filer,
            request: request.digest,
            payment: FeePayment::OnChain {
                txid: output.outpoint.txid.to_byte_array(),
                vout: output.outpoint.vout,
            },
            received_sats: output.sats,
            height: output.height,
        })
    }

    /// Everything that needs the ruling key: the receipt and the
    /// case-accepted record of a paid fee, or the next request.
    fn fee_paperwork(&self, id: &EscrowId, fee: &mut EscrowFee, filers: &[Filer], tip: u32) -> Result<(), String> {
        let record = filers.first().and_then(|filer| filer.record.clone());
        let unlocked = record.is_none() || self.lock.state() == crate::wallet_lock::State::Unlocked;
        if !unlocked {
            return Ok(());
        }
        if let Some(paid) = fee.paid {
            if fee.receipt.is_none() {
                let keys = self.case_keys(record.as_deref())?;
                let receipt = FeeReceipt {
                    moderator_key: keys.record_key().map_err(|err| err.to_string())?,
                    case_id: fee.case_id,
                    request_digest: paid.request,
                    payment: paid.payment,
                    received_sats: paid.received_sats,
                    paid_height: paid.height,
                    due_height: fee.due,
                };
                fee.receipt = Some(keys.sign_fee_receipt(receipt).map_err(|err| err.to_string())?);
                fee.accepted = Some(
                    keys.sign_case_accepted(fee.case_id, paid.height, fee.due)
                        .map_err(|err| err.to_string())?,
                );
            }
            if !fee.receipt_sent {
                let filer = filers
                    .iter()
                    .find(|filer| filer.party == paid.filer)
                    .ok_or("the paying filer's case is gone")?;
                let receipt = fee.receipt.clone().unwrap_or_default();
                self.send_mail(
                    &filer.reply_url,
                    &filer.reply_mail,
                    &Mail::new(Kind::FeeReceipt, *id, vec![receipt]),
                )?;
                fee.receipt_sent = true;
            }
            return Ok(());
        }
        if fee.live(tip).is_some() || tip.saturating_add(REQUEST_MARGIN) > fee.due {
            return Ok(());
        }
        let Some(filer) = next_filer(fee, filers) else {
            return Ok(());
        };
        let script = match fee.scripts.iter().find(|script| script.filer == filer.party) {
            Some(script) => script.script.clone(),
            None => {
                let Some((index, script)) = self.fee_script()? else {
                    return Ok(());
                };
                fee.scripts.push(FeeScript {
                    filer: filer.party,
                    index,
                    script: script.clone(),
                    issued: tip,
                });
                script
            }
        };
        let keys = self.case_keys(filer.record.as_deref())?;
        let invoice = self.ln_fee_invoice(fee.amount, &id_hex(id)[..8]);
        let request = FeeRequest {
            moderator_key: keys.record_key().map_err(|err| err.to_string())?,
            case_id: fee.case_id,
            letter_digest: filer.letter_digest,
            favored: party_byte(filer.party),
            amount_sats: fee.amount,
            invoice: invoice.as_ref().map(|invoice| FeeInvoice {
                network: invoice.network.to_string(),
                bolt11: invoice.bolt11.clone(),
                payment_hash: fixed(&invoice.payment_hash).unwrap_or([0; 32]),
            }),
            script: script.to_bytes(),
            valid_until_height: tip.saturating_add(REQUEST_BLOCKS).min(fee.due - REQUEST_MARGIN),
            due_height: fee.due,
        };
        let bytes = keys.sign_fee_request(request.clone()).map_err(|err| err.to_string())?;
        let digest = SignedFeeRequest::decode(&bytes)
            .and_then(|signed| signed.hash())
            .map_err(|err| err.to_string())?;
        fee.requests.push(Requested {
            filer: filer.party,
            bytes: bytes.clone(),
            digest,
            payment_hash: invoice.map(|invoice| invoice.payment_hash),
            valid_until: request.valid_until_height,
        });
        fee.renew.retain(|party| *party != filer.party);
        self.save_fee(id, fee)?;
        self.send_mail(
            &filer.reply_url,
            &filer.reply_mail,
            &Mail::new(Kind::FeeRequest, *id, vec![bytes]),
        )
        .map(|_| ())
    }

    /// A receive script for a request: one from the pool, else a fresh one
    /// while fewer than [`MAX_UNPAID_SCRIPTS`] are held unused.
    fn fee_script(&self) -> Result<Option<(u32, ScriptBuf)>, String> {
        let pooled = {
            let mut desk = self.moderator_desk();
            let taken = desk.pool.free.pop();
            if taken.is_some() {
                self.escrow.store.save(Role::Moderator, POOL, &desk.pool.to_record())?;
            }
            taken
        };
        if pooled.is_some() {
            return Ok(pooled);
        }
        let held: Vec<ScriptBuf> = self
            .moderator_desk()
            .fees
            .values()
            .filter(|fee| !fee.released)
            .flat_map(|fee| fee.scripts.iter().map(|script| script.script.clone()))
            .collect();
        let mut wallet = self.buyer_wallet();
        if held.iter().filter(|script| !wallet.script_used(script)).count() >= MAX_UNPAID_SCRIPTS {
            return Ok(None);
        }
        wallet.fee_address().map(Some).map_err(|err| err.to_string())
    }

    /// Return the scripts of an escrow past its due height to the pool,
    /// except any that received something. Run a few blocks late, so
    /// a payment confirmed by the due height has been seen.
    fn release_scripts(&self, fee: &mut EscrowFee) {
        let unused: Vec<(u32, ScriptBuf)> = {
            let wallet = self.buyer_wallet();
            fee.scripts
                .iter()
                .filter(|script| !wallet.script_used(&script.script))
                .map(|script| (script.index, script.script.clone()))
                .collect()
        };
        let mut desk = self.moderator_desk();
        desk.pool.free.extend(unused);
        match self.escrow.store.save(Role::Moderator, POOL, &desk.pool.to_record()) {
            Ok(()) => fee.released = true,
            Err(err) => eprintln!("ob-node: fee pool: {err}"),
        }
    }

    #[cfg(test)]
    pub(crate) fn last_fee_request(&self, id: &EscrowId) -> Option<Vec<u8>> {
        self.moderator_desk()
            .fees
            .get(id)?
            .requests
            .last()
            .map(|request| request.bytes.clone())
    }

    /// The public case-accepted records this node's own identity signed, as hex.
    pub(crate) fn cases_accepted_value(&self) -> Vec<Value> {
        let desk = self.moderator_desk();
        desk.fees
            .iter()
            .filter(|(id, _)| {
                desk.cases
                    .iter()
                    .any(|((case_id, _), case)| case_id == *id && case.record.is_some())
            })
            .filter_map(|(_, fee)| fee.accepted.as_deref())
            .take(crate::moderators::MAX_RULINGS)
            .map(|accepted| json!(hex(accepted)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filer(party: Party, received: u32, decided: bool) -> Filer {
        Filer {
            party,
            received,
            letter_digest: [received as u8; 32],
            reply_url: String::new(),
            reply_mail: crate::escrow_mail::key_bytes(&[7; 32]).unwrap(),
            record: None,
            decided,
        }
    }

    fn asked(fee: &mut EscrowFee, party: Party, valid_until: u32) {
        fee.requests.push(Requested {
            filer: party,
            bytes: vec![1],
            digest: [2; 32],
            payment_hash: None,
            valid_until,
        });
    }

    #[test]
    fn the_first_filer_is_asked_first_and_again_only_when_it_writes_again() {
        let mut fee = EscrowFee::new([1; 32], 5_000, 300);
        let filers = [filer(Party::Vendor, 120, false), filer(Party::Buyer, 125, false)];
        assert_eq!(next_filer(&fee, &filers).map(|filer| filer.party), Some(Party::Vendor));
        asked(&mut fee, Party::Vendor, 190);
        assert_eq!((fee.state(Some(190)), fee.state(Some(191))), ("requested", "expired"));
        assert_eq!(
            next_filer(&fee, &filers).map(|filer| filer.party),
            Some(Party::Buyer),
            "then the other filer"
        );
        asked(&mut fee, Party::Buyer, 260);
        assert!(next_filer(&fee, &filers).is_none(), "nobody is asked twice unprompted");
        fee.renew.push(Party::Vendor);
        assert_eq!(next_filer(&fee, &filers).map(|filer| filer.party), Some(Party::Vendor));
        let decided = [filer(Party::Vendor, 120, true)];
        assert!(
            next_filer(&fee, &decided).is_none(),
            "a decided letter is not asked to pay"
        );
    }

    #[test]
    fn a_fee_survives_its_record() {
        let mut fee = EscrowFee::new([1; 32], 5_000, 300);
        fee.scripts.push(FeeScript {
            filer: Party::Buyer,
            index: 4,
            script: ScriptBuf::new_p2a(),
            issued: 120,
        });
        asked(&mut fee, Party::Buyer, 190);
        fee.renew.push(Party::Vendor);
        fee.paid = Some(Paid {
            filer: Party::Buyer,
            request: [2; 32],
            payment: FeePayment::OnChain { txid: [3; 32], vout: 1 },
            received_sats: 6_000,
            height: 150,
        });
        fee.receipt = Some(vec![9]);
        fee.receipt_sent = true;
        fee.refunds.push(Refund {
            filer: Party::Buyer,
            amount: 6_000,
            owed: Some(FeePayment::OnChain { txid: [4; 32], vout: 0 }),
            tx: Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![bitcoin::TxIn::default()],
                output: vec![bitcoin::TxOut {
                    value: bitcoin::Amount::from_sat(1),
                    script_pubkey: bitcoin::ScriptBuf::new(),
                }],
            },
        });
        assert_eq!(EscrowFee::from_record(&fee.to_record()), Some(fee.clone()));
        fee.paid.as_mut().unwrap().payment = FeePayment::Lightning { payment_hash: [5; 32] };
        assert_eq!(EscrowFee::from_record(&fee.to_record()), Some(fee));
        let pool = FeePool {
            free: vec![(4, ScriptBuf::new_p2a())],
        };
        assert_eq!(FeePool::from_record(&pool.to_record()).unwrap().free, pool.free);
        assert_eq!(fee_id(&fee_name(&[6; 16])), Some([6; 16]));
        assert_eq!(fee_id(&format!("{}-buyer", id_hex(&[6; 16]))), None);
    }
}
