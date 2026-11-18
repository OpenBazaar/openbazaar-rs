//! The buyer's filing-fee mail, and the fee mirrored onto its order
//! ([`crate::escrow_dispute_fee`]).

use openbazaar_escrow::{Party, SignedTerms};
use serde_json::{json, Value};

use super::{failed, text};
use crate::buyer_orders::BuyerOrder;
use crate::escrow_dispute_fee::FiledEscrow;
use crate::escrow_mail::{unhex, EscrowId, Mail};
use crate::Node;

fn filed(order: &BuyerOrder) -> Result<FiledEscrow, String> {
    let escrow = order.escrow.as_ref().ok_or("that order has no escrow")?;
    let signed = SignedTerms::decode(&unhex(text(escrow, "terms")?)?).map_err(failed)?;
    Ok(FiledEscrow {
        terms: signed.terms().clone(),
        outpoint: text(escrow, "outpoint")?.parse().map_err(failed)?,
    })
}

impl Node {
    pub(crate) fn buyer_filed_escrow(&self, id: &EscrowId) -> Result<FiledEscrow, String> {
        filed(&self.escrow_order(id)?)
    }

    pub(crate) fn mirror_buyer_fee(&self, id: &EscrowId, value: Value) -> Result<(), String> {
        let order = self.escrow_order(id)?;
        self.update_escrow(&order.id, None, json!({ "disputeFee": value }))
    }

    pub(super) fn buyer_fee_mail(&self, order: &BuyerOrder, mail: &Mail) -> Result<(), String> {
        self.filer_mail(Party::Buyer, &filed(order)?, mail)
    }

    pub(super) fn note_buyer_letter(&self, order: &BuyerOrder, id: &EscrowId, letter: &[u8]) -> Result<Option<Vec<u8>>, String> {
        self.note_dispute_letter(Party::Buyer, id, &filed(order)?.terms, letter)
    }
}
