//! How to pay one seller order.
//!
//! Direct pay: a fresh receive address from that seller's own wallet (the
//! rail the watcher follows), a BIP 21 URI, the seller's BIP 352 code, and a
//! BOLT 12 offer for the amount. The code and the offer are shown but not
//! watched: this node does not scan for silent payments or run Lightning.
//!
//! Moderated pay (testnet4 only, an unsafe demo): the MuSig2 funding address
//! for a fresh buyer escrow key and a fresh vendor escrow key, both derived
//! from this node's seeds. The address is not safe to fund from another
//! wallet. Funding goes through `POST /orders/{id}/pay`, which presigns the
//! refund for the real outpoint before it broadcasts. Mainnet refuses escrow.
//! The escrow crate's API is expected to change (two-party funding), so this
//! module is the only place the node builds escrow terms.

use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{Address, Amount, OutPoint};
use openbazaar_escrow::{fund, FundParams};
use openbazaar_pay::{Bolt12Offer, DirectPay, SilentPaymentCode};
use serde_json::{json, Value};

use crate::network::{bip21, NetworkMode};
use crate::{Node, OrderError};

pub const ESCROW_MAINNET_REFUSAL: &str =
    "Moderated escrow is not available on mainnet. It is a testnet4 demo until the two-party escrow protocol ships.";

pub const ESCROW_DEMO_WARNING: &str = "Unsafe demo: testnet4 coins only. This node holds both the buyer and the vendor escrow keys and runs the ceremony in one process, the moderator key is a public fixture, and the escrow protocol is about to change. Do not send anything of value.";

/// Blocks from funding until the presigned refund can be mined (about 30 days).
pub const ESCROW_TIMEOUT_BLOCKS: u32 = 4_320;

/// Sats the buyer adds on top of the price so the escrow spend can pay its fee.
pub const ESCROW_SPEND_FEE_SATS: u64 = 500;

impl Node {
    pub(crate) fn direct_payment(&self, total: u64, seller: &str) -> Result<Value, OrderError> {
        let network = self.network;
        let mut wallet = self.store_wallet(seller)?;
        let address = wallet.fresh_address().map_err(failed)?;
        let seed = &wallet.seed;
        let code = SilentPaymentCode::from_keys(&seed.scan_key(), &seed.silent_spend_key(), network.bitcoin())
            .map_err(failed)?;
        let signing = PublicKey::from_secret_key(&Secp256k1::new(), &seed.store_signing_key());
        let offer = Bolt12Offer::create(signing, Amount::from_sat(total), seller, network.bitcoin())
            .map_err(failed)?;
        let pay = DirectPay::new(Amount::from_sat(total), offer, code).map_err(failed)?;
        Ok(json!({
            "network": network.as_str(),
            "onchain": address.to_string(),
            "address": address.to_string(),
            "amountSats": total,
            "bip21": bip21(&address, Amount::from_sat(total)),
            "lightning": pay.offer().to_string(),
            "silentPaymentCode": pay.silent_payment_code().to_string(),
            "watched": ["onchain"],
        }))
    }

    pub(crate) fn escrow_terms(&self, total: u64, seller: &str) -> Result<Value, OrderError> {
        let network = self.network;
        if network != NetworkMode::Testnet4 {
            return Err(OrderError::Invalid(ESCROW_MAINNET_REFUSAL));
        }
        let funding = total
            .checked_add(ESCROW_SPEND_FEE_SATS)
            .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
        let (buyer_index, buyer_secret, buyer_payout) = {
            let mut wallet = self.buyer_wallet();
            let index = wallet.next_escrow_index().map_err(failed)?;
            let secret = wallet.seed.escrow_key(index).secret_bytes();
            (index, secret, wallet.fresh_address().map_err(failed)?)
        };
        let (vendor_index, vendor_secret, vendor_payout) = {
            let mut wallet = self.store_wallet(seller)?;
            let index = wallet.next_escrow_index().map_err(failed)?;
            let secret = wallet.seed.escrow_key(index).secret_bytes();
            (index, secret, wallet.fresh_address().map_err(failed)?)
        };
        // The funding script depends only on the two keys. The outpoint and
        // timeout here are placeholders; `POST /orders/{id}/pay` runs the
        // ceremony again for the real outpoint before anything is sent.
        let preview = fund(FundParams {
            buyer_secret,
            vendor_secret,
            moderator_public: PublicKey::from_secret_key(&Secp256k1::new(), &self.keys.moderator).serialize(),
            funding_outpoint: OutPoint::null(),
            funding_value: Amount::from_sat(funding),
            fee: Amount::from_sat(ESCROW_SPEND_FEE_SATS),
            timeout_height: 1,
            buyer_payout: buyer_payout.script_pubkey(),
            vendor_payout: vendor_payout.script_pubkey(),
        })
        .map_err(failed)?;
        let address = Address::from_script(&preview.funding_output().script_pubkey, network.bitcoin())
            .map_err(failed)?;
        Ok(json!({
            "network": network.as_str(),
            "demo": true,
            "unsafe": true,
            "warning": ESCROW_DEMO_WARNING,
            "fundingSats": funding,
            "spendFeeSats": ESCROW_SPEND_FEE_SATS,
            "address": address.to_string(),
            "timeoutHeight": 0,
            "timeoutBlocks": ESCROW_TIMEOUT_BLOCKS,
            "outpoint": "",
            "signaturesRequired": 2,
            "buyerKeyIndex": buyer_index,
            "vendorKeyIndex": vendor_index,
            "buyerPayout": buyer_payout.to_string(),
            "vendorPayout": vendor_payout.to_string(),
            "refundHeld": false,
            "safeToSend": false,
            "fundWith": "node",
        }))
    }
}

fn failed(err: impl std::fmt::Display) -> OrderError {
    OrderError::Failed(err.to_string())
}
