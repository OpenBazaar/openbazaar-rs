//! The vendor's sale on disk, and the live escrow restored from it.

use bitcoin::{OutPoint, Txid};
use openbazaar_escrow::{EscrowTerms, ReleaseNonce, VendorAwaitingSignatures, VendorEscrow};
use serde_json::{json, Value};
use tormail::StoreKey;

use super::{failed, VendorDesk};
use crate::escrow_mail::{hex, key_bytes, parse_id, unhex, EscrowId};
use crate::escrow_store::{self, EscrowStore, Role};
use crate::Node;

/// Everything the vendor knows about one escrow. Stored after every change
/// and before anything it produced is sent.
pub(super) struct Sale {
    pub seller: String,
    pub index: u32,
    pub terms: EscrowTerms,
    pub buyer_mail: StoreKey,
    pub buyer_url: String,
    pub stage: String,
    /// The Offer fields, sent again for a repeated request.
    pub offer: Vec<Vec<u8>>,
    pub outpoint: Option<OutPoint>,
    pub open_attempt: u32,
    /// The `VendorReply` to `open_attempt`, sent again for a repeated open.
    pub reply: Option<Vec<u8>>,
    /// `VendorAwaitingSignatures` snapshot while replied.
    pub awaiting: Option<Vec<u8>>,
    /// `VendorEscrow` snapshot from funding on.
    pub escrow: Option<Vec<u8>>,
    pub claim: Option<Vec<u8>>,
    /// A signed outcome (release or dispute) not yet known to be broadcast.
    pub outcome_tx: Option<String>,
    pub release_txid: Option<Txid>,
    pub settle_txid: Option<Txid>,
    pub error: Option<String>,
}

fn opt_hex(bytes: &Option<Vec<u8>>) -> Value {
    bytes.as_deref().map(hex).into()
}

fn read_hex(value: &Value) -> Option<Option<Vec<u8>>> {
    match value.as_str() {
        Some(text) => Some(Some(unhex(text).ok()?)),
        None => Some(None),
    }
}

impl Sale {
    fn to_record(&self) -> Value {
        json!({
            "seller": self.seller,
            "index": self.index,
            "terms": hex(&self.terms.encode()),
            "buyerMail": hex(self.buyer_mail.as_bytes()),
            "buyerUrl": self.buyer_url,
            "stage": self.stage,
            "offer": self.offer.iter().map(|field| hex(field)).collect::<Vec<_>>(),
            "outpoint": self.outpoint.map(|outpoint| outpoint.to_string()),
            "openAttempt": self.open_attempt,
            "reply": opt_hex(&self.reply),
            "awaiting": opt_hex(&self.awaiting),
            "escrow": opt_hex(&self.escrow),
            "claim": opt_hex(&self.claim),
            "outcomeTx": self.outcome_tx,
            "releaseTxid": self.release_txid.map(|txid| txid.to_string()),
            "settleTxid": self.settle_txid.map(|txid| txid.to_string()),
            "error": self.error,
        })
    }

    fn from_record(value: &Value) -> Option<Self> {
        let txid = |key: &str| match value[key].as_str() {
            Some(text) => text.parse().ok().map(Some),
            None => Some(None),
        };
        Some(Self {
            seller: value["seller"].as_str()?.to_string(),
            index: u32::try_from(value["index"].as_u64()?).ok()?,
            terms: EscrowTerms::decode(&unhex(value["terms"].as_str()?).ok()?).ok()?,
            buyer_mail: key_bytes(&unhex(value["buyerMail"].as_str()?).ok()?).ok()?,
            buyer_url: value["buyerUrl"].as_str()?.to_string(),
            stage: value["stage"].as_str()?.to_string(),
            offer: value["offer"]
                .as_array()?
                .iter()
                .map(|field| unhex(field.as_str()?).ok())
                .collect::<Option<_>>()?,
            outpoint: match value["outpoint"].as_str() {
                Some(text) => Some(text.parse().ok()?),
                None => None,
            },
            open_attempt: u32::try_from(value["openAttempt"].as_u64()?).ok()?,
            reply: read_hex(&value["reply"])?,
            awaiting: read_hex(&value["awaiting"])?,
            escrow: read_hex(&value["escrow"])?,
            claim: read_hex(&value["claim"])?,
            outcome_tx: value["outcomeTx"].as_str().map(str::to_string),
            release_txid: txid("releaseTxid")?,
            settle_txid: txid("settleTxid")?,
            error: value["error"].as_str().map(str::to_string),
        })
    }
}

/// The funded escrow and the release nonce it has out. Memory only: a
/// nonce never survives a restart.
pub(super) struct Live {
    pub escrow: VendorEscrow,
    pub nonce: Option<ReleaseNonce>,
}

impl VendorDesk {
    pub(crate) fn load(store: &EscrowStore) -> Self {
        let mut desk = Self::default();
        for (name, record) in store.list(Role::Vendor) {
            match (parse_id(&name), Sale::from_record(&record)) {
                (Some(id), Some(sale)) => {
                    desk.sales.insert(id, sale);
                }
                _ => eprintln!("ob-node: vendor sale {name} could not be read"),
            }
        }
        desk
    }
}

impl Node {
    /// Store `sale` and put it in the desk.
    pub(super) fn save_sale(&self, id: &EscrowId, sale: Sale) -> Result<(), String> {
        self.escrow
            .store
            .save(Role::Vendor, &escrow_store::name(id), &sale.to_record())?;
        self.vendor_desk().sales.insert(*id, sale);
        Ok(())
    }

    /// Change the stored sale with `edit`, then store it.
    pub(super) fn update_sale(&self, id: &EscrowId, edit: impl FnOnce(&mut Sale)) -> Result<(), String> {
        let mut desk = self.vendor_desk();
        let sale = desk.sales.get_mut(id).ok_or("no sale for that escrow")?;
        edit(sale);
        self.store_or_revert(id, sale)
    }

    /// Store `sale`, or put back what is on disk if that fails.
    fn store_or_revert(&self, id: &EscrowId, sale: &mut Sale) -> Result<(), String> {
        let name = escrow_store::name(id);
        let saved = self.escrow.store.save(Role::Vendor, &name, &sale.to_record());
        if saved.is_err() {
            if let Some(stored) = self.escrow.store.load(Role::Vendor, &name).ok().flatten().as_ref().and_then(Sale::from_record) {
                *sale = stored;
            }
        }
        saved
    }

    pub(super) fn restore_awaiting(&self, id: &EscrowId) -> Result<VendorAwaitingSignatures, String> {
        let (seller, index, bytes) = {
            let desk = self.vendor_desk();
            let sale = desk.sales.get(id).ok_or("no sale for that escrow")?;
            (sale.seller.clone(), sale.index, sale.awaiting.clone().ok_or("that sale has no open round")?)
        };
        let secret = self.vendor_secret(&seller, index)?;
        VendorAwaitingSignatures::restore(&bytes, &secret, &self.escrow_context()?).map_err(failed)
    }

    /// Run `step` on the funded escrow, restoring it from its snapshot if
    /// this process does not hold it. A changed phase is stored before this
    /// returns, together with `record` applied to the sale.
    pub(super) fn with_vendor_escrow<T>(
        &self,
        id: &EscrowId,
        step: impl FnOnce(&mut Live) -> Result<T, String>,
        record: impl FnOnce(&mut Sale, &T),
    ) -> Result<T, String> {
        if !self.vendor_desk().live.contains_key(id) {
            let (seller, index, bytes) = {
                let desk = self.vendor_desk();
                let sale = desk.sales.get(id).ok_or("no sale for that escrow")?;
                (sale.seller.clone(), sale.index, sale.escrow.clone().ok_or("that sale is not funded")?)
            };
            let secret = self.vendor_secret(&seller, index)?;
            let escrow = VendorEscrow::restore(&bytes, &secret, &self.escrow_context()?).map_err(failed)?;
            self.vendor_desk().live.entry(*id).or_insert(Live { escrow, nonce: None });
        }
        let mut desk = self.vendor_desk();
        let live = desk.live.get_mut(id).ok_or("the escrow is gone")?;
        let before = live.escrow.phase();
        let out = step(live);
        let snapshot = (live.escrow.phase() != before).then(|| live.escrow.snapshot());
        let out = match out {
            Ok(out) => out,
            Err(err) => {
                if snapshot.is_some() {
                    desk.live.remove(id);
                }
                return Err(err);
            }
        };
        let sale = desk.sales.get_mut(id).ok_or("no sale for that escrow")?;
        if let Some(snapshot) = snapshot {
            sale.escrow = Some(snapshot);
        }
        record(sale, &out);
        let saved = self.store_or_revert(id, sale);
        if saved.is_err() {
            // Memory is ahead of the disk; drop it so the next step restores.
            desk.live.remove(id);
        }
        saved.map(|()| out)
    }
}
