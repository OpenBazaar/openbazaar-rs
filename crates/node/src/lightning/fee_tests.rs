//! Dispute filing fees between two nodes on linked mock Lightning nodes.

use std::sync::Arc;

use bitcoin::hashes::{sha256, Hash};
use openbazaar_lightning::ldk_node::bitcoin::Network;
use openbazaar_lightning::mock::MockRail;

use super::fee::FeePaid;
use super::sale_tests::arm;
use crate::multinode_seller_tests::Pair;

#[test]
fn a_fee_invoice_needs_channel_room_and_its_payment_proves_itself() {
    let pair = Pair::new();
    let moderator_rail = Arc::new(MockRail::new(Network::Signet));
    let filer_rail = Arc::new(moderator_rail.linked());
    filer_rail.set_lightning_sats(100_000);
    arm(&pair.seller.node, &moderator_rail);
    arm(&pair.buyer.node, &filer_rail);
    let (moderator, filer) = (&pair.seller.node, &pair.buyer.node);

    assert_eq!(moderator.ln_fee_invoice(5_000, "case"), None, "no channel room: on-chain only");
    moderator_rail.set_inbound_sats(4_999);
    assert_eq!(moderator.ln_fee_invoice(5_000, "case"), None);
    moderator_rail.set_inbound_sats(5_000);
    let invoice = moderator.ln_fee_invoice(5_000, "case").expect("room for the fee");
    assert_eq!(invoice.network, "signet");

    let refused = [
        filer.ln_can_pay_fee("testnet4", &invoice.bolt11, &invoice.payment_hash, 5_000),
        filer.ln_can_pay_fee("signet", &invoice.bolt11, &invoice.payment_hash, 5_001),
        filer.ln_can_pay_fee("signet", &invoice.bolt11, &"00".repeat(32), 5_000),
        moderator.ln_can_pay_fee("signet", &invoice.bolt11, &invoice.payment_hash, 5_000),
    ];
    for result in refused {
        assert!(result.is_err(), "{result:?}");
    }
    let cap = filer.ln_can_pay_fee("signet", &invoice.bolt11, &invoice.payment_hash, 5_000).unwrap();
    assert_eq!(moderator.ln_fee_received(&invoice.payment_hash), None);
    let hash = filer.ln_pay_fee(&invoice.bolt11, cap).unwrap();
    assert_eq!(hash, invoice.payment_hash);
    assert_eq!(moderator.ln_fee_received(&invoice.payment_hash), Some(5_000_000));
    let FeePaid::Paid { preimage: Some(preimage), .. } = filer.ln_fee_paid(&hash) else {
        panic!("paid, with a preimage");
    };
    assert_eq!(sha256::Hash::hash(&preimage).to_string(), invoice.payment_hash);
}
