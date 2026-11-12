//! New-seller escrow and the store's reputation bond.
//!
//! Finished escrow sales are payouts (a release or a dispute settlement),
//! not a direct pay and not a sale the seller paid to itself. While a store
//! has none, an escrow cannot start without a passing bond. Direct pay is
//! not held back: a new store takes it, and the buyer's client warns before
//! paying. The count is a number. Buyer keys and orders are not published.

use std::fs;

use bitcoin::secp256k1::{PublicKey, Secp256k1};
use openbazaar_protocol::{bond_script, accept_bond, Bond, BondLock, Signed, NETWORK_TESTNET4, MIN_BOND_LOCK, MIN_BOND_SATS};
use serde_json::{json, Value};

use crate::escrow_mail::hex;
use crate::instructions::ESCROW_MAINNET_REFUSAL;
use crate::network::NetworkMode;
use crate::Node;

pub(crate) const NEW_SELLER_BOND: &str = "A new seller posts a reputation bond before an escrow sale.";

/// One row the finished-sale count looks at. A direct pay is never a row
/// the vendor desk stores; `direct` is here so a direct pay cannot be counted.
#[derive(Clone, Copy)]
pub(crate) struct Finish {
    pub payout: bool,
    pub self_pay: bool,
    pub direct: bool,
}

pub(crate) fn counts_toward_finished(row: Finish) -> bool {
    row.payout && !row.self_pay && !row.direct
}

pub(crate) fn finished_escrow_count(rows: &[Finish]) -> u32 {
    rows.iter().copied().filter(|row| counts_toward_finished(*row)).count() as u32
}

pub(crate) fn escrow_start_allowed(finished: u32, bond_ok: bool) -> Result<(), &'static str> {
    if finished == 0 && !bond_ok {
        Err(NEW_SELLER_BOND)
    } else {
        Ok(())
    }
}

impl Node {
    fn bond_path(&self) -> std::path::PathBuf {
        self.data_dir.join("reputation-bond")
    }

    /// A bond this node stored that a buyer would accept.
    pub(crate) fn passing_bond(&self) -> bool {
        if self.network != NetworkMode::Testnet4 {
            return false;
        }
        let Ok(text) = fs::read_to_string(self.bond_path()) else {
            return false;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return false;
        };
        let Some(record) = value.get("record").and_then(Value::as_str) else {
            return false;
        };
        let Ok(bytes) = crate::escrow_mail::unhex(record) else {
            return false;
        };
        let Ok(bond) = openbazaar_protocol::SignedBond::decode(&bytes) else {
            return false;
        };
        bond.verify().is_ok() && accept_bond(&bytes, &bond.terms.txid, bond.terms.vout).is_ok()
    }

    /// Sign and store a testnet4 bond for `seller`'s store key.
    pub(crate) fn publish_store_bond(&self, seller: &str) -> Result<(), String> {
        if self.network != NetworkMode::Testnet4 {
            return Err(ESCROW_MAINNET_REFUSAL.into());
        }
        let secret = self.seller_store_secret(seller)?;
        let store_pubkey = PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize();
        let lock = BondLock::Cltv(MIN_BOND_LOCK);
        let script = bond_script(&store_pubkey, lock).map_err(|err| err.to_string())?.to_bytes();
        let signed = Signed::sign(
            Bond {
                network: NETWORK_TESTNET4,
                store_pubkey,
                txid: [0xbd; 32],
                vout: 0,
                amount_sats: MIN_BOND_SATS,
                lock,
                script,
            },
            &secret,
        )
        .map_err(|err| err.to_string())?;
        let bytes = signed.encode().map_err(|err| err.to_string())?;
        fs::write(self.bond_path(), serde_json::to_vec(&json!({ "record": hex(&bytes) })).map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;
        if !self.passing_bond() {
            return Err("the bond did not pass the buyer's check".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_with_zero_finished_escrow_sales_starts_escrow_only_with_a_bond() {
        assert_eq!(escrow_start_allowed(0, false), Err(NEW_SELLER_BOND));
        assert!(escrow_start_allowed(0, true).is_ok(), "a passing bond lets the first escrow start");
    }

    #[test]
    fn a_store_with_one_finished_escrow_sale_is_not_asked_for_a_bond() {
        assert!(escrow_start_allowed(1, false).is_ok(), "an established store is not asked for a bond");
    }

    #[test]
    fn a_direct_pay_is_not_counted_as_a_finished_escrow_sale() {
        let rows = [
            Finish { payout: true, self_pay: false, direct: true },
            Finish { payout: true, self_pay: true, direct: false },
            Finish { payout: false, self_pay: false, direct: false },
        ];
        assert_eq!(finished_escrow_count(&rows), 0);
        assert_eq!(
            finished_escrow_count(&[Finish { payout: true, self_pay: false, direct: false }]),
            1
        );
    }

    #[test]
    fn a_new_seller_without_a_passing_bond_is_refused_at_escrow_start() {
        assert_eq!(escrow_start_allowed(0, false), Err(NEW_SELLER_BOND));
    }
}
