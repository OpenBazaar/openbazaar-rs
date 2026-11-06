//! How to pay one seller order.
//!
//! Direct pay: a fresh receive address from that seller's own wallet (the
//! rail the watcher follows), a BIP 21 URI, the seller's BIP 352 code, and a
//! BOLT 12 offer for the amount. The code and the offer are shown but not
//! watched: this node does not scan for silent payments or run Lightning.
//!
//! Moderated pay (testnet4 only, an unsafe demo) is two-party: see
//! `escrow_buyer` and `escrow_vendor`. Checkout records a fresh buyer key and
//! payout; the funding address arrives with the vendor node's terms.
//! Mainnet refuses escrow.

use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::Amount;
use openbazaar_pay::{Bolt12Offer, DirectPay, SilentPaymentCode};
use serde_json::{json, Value};

use crate::network::bip21;
use crate::{Node, OrderError};

pub const ESCROW_MAINNET_REFUSAL: &str =
    "Moderated escrow is not available on mainnet. It is a testnet4 demo until the two-party escrow protocol is reviewed.";

pub const ESCROW_DEMO_WARNING: &str = "Unsafe demo: testnet4 coins only. The moderator and the vendor mailbox keys are public fixtures, escrow messages travel over unauthenticated loopback HTTP, and a node restart after funding keeps only the presigned refund. Do not send anything of value.";

/// Network fee every presigned escrow spend carries. The buyer adds it to the price.
pub const ESCROW_SPEND_FEE_SATS: u64 = 1_000;

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
}

fn failed(err: impl std::fmt::Display) -> OrderError {
    OrderError::Failed(err.to_string())
}
