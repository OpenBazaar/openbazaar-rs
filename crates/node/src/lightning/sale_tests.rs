//! A Lightning sale between two nodes, on mock Lightning nodes that share a ledger.

use std::sync::Arc;

use openbazaar_lightning::ldk_node::bitcoin::Network;
use openbazaar_lightning::mock::MockRail;
use openbazaar_lightning::LightningRail;
use serde_json::{json, Value};

use super::pay::fee_cap;
use crate::harness::{order, request, ship_to, PASSPHRASE};
use crate::lightning::LnFlags;
use crate::multinode_seller_tests::Pair;
use crate::network::NetworkMode;
use crate::watch::{AWAITING_PAYMENT, AWAITING_SELLER, PAID};

const PRICE: u64 = 28_000;

pub(super) fn arm(node: &crate::Node, rail: &Arc<MockRail>) {
    let mut flags = LnFlags::default();
    let args = [
        "--lightning",
        "--ln-chain",
        "bitcoind:u:p@127.0.0.1:18443",
        "--lsp",
        "0371d6fd7d75de2d0372d03ea00e8bacdacb50c27d0eaea0a76a0622eff1f5ef2b@127.0.0.1:9735",
        "--ln-p2p-gossip",
    ];
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        flags.take(flag, |_| iter.next().map(|text| text.to_string()).ok_or_else(|| "needs a value".to_string())).unwrap().unwrap();
    }
    node.enable_lightning(flags.plan(NetworkMode::Testnet4, None).unwrap().unwrap()).unwrap();
    node.set_lightning(rail.clone()).unwrap();
}

fn listing(node: &crate::Node) -> Value {
    let (_, catalog) = request(node, "GET", "/catalog", "");
    catalog["listings"].as_array().unwrap().iter().find(|listing| listing["seller"] == "Brook and Pine").cloned().unwrap()
}

#[test]
fn a_remote_lightning_sale_invoices_on_the_seller_and_the_buyer_pays_it() {
    let pair = Pair::new();
    let seller_rail = Arc::new(MockRail::new(Network::Signet));
    let buyer_rail = Arc::new(seller_rail.linked());
    buyer_rail.set_lightning_sats(1_000_000);
    arm(&pair.seller.node, &seller_rail);
    arm(&pair.buyer.node, &buyer_rail);

    let own = pair.seller.node.own_record().expect("published").name;
    let (status, stores) = request(
        &pair.seller.node,
        "PUT",
        "/ln/stores",
        &json!({ "seller": own, "accept": true, "acknowledgeLsp": true }).to_string(),
    );
    assert!(status.contains("200"), "{status} {stores}");
    pair.buyer.node.replicate_once();

    let bought = listing(&pair.buyer.node);
    assert_eq!(bought["acceptsLightning"], true, "{bought}");
    let package = json!({ "settlement": "direct", "rail": "ln", "lines": [{ "listingHash": bought["hash"], "quantity": 1 }] });
    let body = json!({ "shipTo": ship_to(), "packages": [package.clone()] }).to_string();
    let (status, refused) = request(&pair.seller.node, "POST", "/checkout", &body);
    assert!(status.contains("400") && refused["error"].as_str().unwrap_or("").contains("own store"), "{status} {refused}");

    let (status, placed) = request(&pair.buyer.node, "POST", "/checkout", &body);
    assert!(status.contains("200"), "{status} {placed}");
    assert_eq!(placed["orders"][0]["status"], AWAITING_SELLER);
    assert!(placed["orders"][0].get("payment").is_none(), "the seller names the invoice");
    pair.deliver();

    let id = placed["orders"][0]["orderId"].as_str().unwrap();
    let bought = order(&pair.buyer.node, id);
    let payment = &bought["payment"];
    assert_eq!((&bought["status"], &bought["totalSats"]), (&json!(AWAITING_PAYMENT), &json!(PRICE)));
    assert_eq!(payment["rail"], "ln");
    assert_eq!(payment["issuer"], "remote");
    assert!(payment.get("address").is_none(), "no bitcoin address: {payment}");
    let hash = payment["paymentHash"].as_str().unwrap().to_string();
    let invoice = payment["invoice"].as_str().unwrap();
    assert!(invoice.starts_with("ln"), "{invoice}");
    assert_eq!(payment["amountMsat"], PRICE * 1_000);

    let sale = pair.sales().remove(0);
    assert_eq!((&sale["status"], &sale["address"], &sale["payment"]["paymentHash"]), (&json!(AWAITING_PAYMENT), &Value::Null, &json!(hash)));
    assert_eq!(sale["payment"]["invoice"], invoice);

    let (status, quote) = request(&pair.buyer.node, "POST", &format!("/ln/orders/{id}/pay/quote"), "");
    assert!(status.contains("200"), "{status} {quote}");
    assert_eq!(quote["feeCapMsat"], fee_cap(PRICE * 1_000));
    assert_eq!(quote["paymentHash"], hash);
    let (status, ticket) = request(
        &pair.buyer.node,
        "POST",
        "/ln/spend-tickets",
        &json!({ "quote": quote["quote"], "passphrase": PASSPHRASE }).to_string(),
    );
    assert!(status.contains("200"), "{status} {ticket}");
    let (status, paid) = request(
        &pair.buyer.node,
        "POST",
        &format!("/ln/orders/{id}/pay"),
        &json!({ "ticket": ticket["ticket"] }).to_string(),
    );
    assert!(status.contains("202"), "{status} {paid}");
    assert_eq!(paid["status"], PAID);
    assert_eq!(paid["payment"]["paymentHash"], hash);
    assert!(matches!(seller_rail.incoming(&hash), openbazaar_lightning::Incoming::Paid { .. }));

    assert_eq!(pair.seller.node.ln_update_sales(), 1);
    let sale = pair.sales().remove(0);
    assert_eq!(sale["status"], PAID);
    assert_eq!(sale["txids"][0], json!(hash));
    pair.deliver();
    let told = order(&pair.buyer.node, id);
    assert_eq!(told["status"], PAID);
    assert_eq!(told["remote"]["sellerSawPayment"], true);
}
