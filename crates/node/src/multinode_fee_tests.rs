//! The dispute filing fee on the moderator's desk (v5 terms), on the
//! three-node harness: the request goes out once a letter passes review,
//! the operator can decide only once the fee is paid, and a denial is final.

use bitcoin::absolute::LockTime;
use bitcoin::{Address, Network, Script};
use openbazaar_escrow::{read_dispute_decision, Decision, Party, SignedTerms, ANCHOR_VALUE};
use openbazaar_protocol::{FeeRequest, SignedFeeRequest, FAVORED_BUYER};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, parse_id, unhex};
use crate::harness::{fund_node_wallet, hash_of, order, request, ship_to, unlock, Market, PASSPHRASE};
use crate::multinode_tests::{funded, funding_tx, sale, spends};
use crate::{Node, SETTLED};

const SELLER: &str = "Marlow Goods";
pub(crate) const FEE: u64 = 2_500;

/// The moderator publishes with `fee`, both parties accept it, and the
/// buyer funds an escrow under it. Returns the order id and the terms.
pub(crate) fn funded_with(market: &Market, fee: u64) -> (String, Value) {
    let body = json!({ "name": "Ines Varga", "feeSats": fee });
    let (status, value) = request(&market.moderator.node, "PUT", "/moderator/profile", &body.to_string());
    assert!(status.contains("200"), "{status} {value}");
    let chosen = value["id"].as_str().unwrap().to_string();
    for node in [&market.vendor.node, &market.buyer.node] {
        let (status, value) = request(
            node,
            "POST",
            "/moderators",
            &json!({ "address": market.moderator.url }).to_string(),
        );
        assert!(status.contains("200"), "{status} {value}");
    }
    fund_node_wallet(&market.buyer.node, &market.chain, 100_000);
    let package = json!({ "settlement": "escrow", "moderator": chosen, "lines": [{ "listingHash": hash_of(&market.buyer.node, "Paperback novel", SELLER), "quantity": 1 }] });
    let (status, placed) = request(
        &market.buyer.node,
        "POST",
        "/checkout",
        &json!({ "shipTo": ship_to(), "packages": [package] }).to_string(),
    );
    assert!(status.contains("200"), "{status} {placed}");
    let id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();
    market.deliver();
    let escrow = funded(market, &id)["escrow"].clone();
    market.mine();
    (id, escrow)
}

pub(crate) fn terms(escrow: &Value) -> openbazaar_escrow::EscrowTerms {
    SignedTerms::decode(&unhex(escrow["terms"].as_str().unwrap()).unwrap())
        .unwrap()
        .terms()
        .clone()
}

/// The buyer files; returns the moderator's case for it.
pub(crate) fn filed(market: &Market, id: &str) -> Value {
    let (status, body) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/dispute"),
        r#"{"memo": "The box arrived empty."}"#,
    );
    assert!(status.contains("200"), "{status} {body}");
    market.deliver();
    case(&market.moderator.node)
}

pub(crate) fn case(node: &Node) -> Value {
    request(node, "GET", "/moderator/disputes", "").1["disputes"][0].clone()
}

pub(crate) fn act(node: &Node, case: &Value, action: &str, favor: &str) -> (String, Value) {
    let path = format!("/moderator/disputes/{}/{action}", case["id"].as_str().unwrap());
    request(node, "POST", &path, &json!({ "favor": favor }).to_string())
}

pub(crate) fn last_request(node: &Node, case: &Value) -> Option<FeeRequest> {
    let bytes = node.last_fee_request(&parse_id(case["id"].as_str().unwrap()).unwrap())?;
    Some(SignedFeeRequest::decode(&bytes).unwrap().terms)
}

/// Pay the request on-chain from outside, confirm it, and let the desk see it.
pub(crate) fn pay(market: &Market, request: &FeeRequest) {
    let script = Script::from_bytes(&request.script);
    let address = Address::from_script(script, Network::Testnet4).unwrap();
    let tx = market.chain.send_to_wallet_locked(&address, request.amount_sats, true, LockTime::ZERO);
    market.chain.confirm_next(tx.compute_txid());
    market.mine();
    market.moderator.node.moderator_fee_steps();
}

#[test]
fn a_moderator_rules_only_once_the_filing_fee_is_paid_and_takes_no_cut_of_the_escrow() {
    let mut market = Market::new();
    let (id, escrow) = funded_with(&market, FEE);
    let terms = terms(&escrow);
    assert_eq!(
        (terms.version(), terms.moderator.filing_fee().map(|fee| fee.to_sat())),
        (5, Some(FEE)),
        "a v5 escrow"
    );
    let moderator = market.moderator.node.clone();
    let filed = filed(&market, &id);
    assert_eq!(filed["fee"]["state"], "awaiting-request", "{filed}");
    assert_eq!(filed["fee"]["dueHeight"], terms.vendor_claim_height);
    let (status, refused) = act(&moderator, &filed, "rule", "buyer");
    assert!(
        status.contains("409") && refused["error"].as_str().unwrap().contains("not paid"),
        "{status} {refused}"
    );

    moderator.moderator_fee_steps();
    let asked = last_request(&moderator, &filed).expect("a request once the letter passed review");
    assert_eq!(
        (
            asked.amount_sats,
            asked.favored,
            asked.due_height,
            asked.invoice.is_none()
        ),
        (FEE, FAVORED_BUYER, terms.vendor_claim_height, true)
    );
    assert_eq!(json!(hex(&asked.case_id)), filed["caseId"]);
    assert!(asked.valid_until_height > market.chain.tip() && asked.valid_until_height <= terms.vendor_claim_height);
    assert_eq!(case(&moderator)["fee"]["state"], "requested");
    moderator.moderator_fee_steps();
    assert_eq!(
        last_request(&moderator, &filed),
        Some(asked.clone()),
        "a live request is not issued again"
    );
    let (status, refused) = act(&moderator, &filed, "deny", "buyer");
    assert!(
        status.contains("409"),
        "an unpaid letter is not denied either: {status} {refused}"
    );

    pay(&market, &asked);
    let paid = case(&moderator)["fee"].clone();
    assert_eq!(
        (&paid["state"], &paid["rail"], &paid["paidBy"]),
        (&json!("paid"), &json!("onchain"), &json!("buyer")),
        "{paid}"
    );
    assert_eq!(
        moderator.cases_accepted_value().len(),
        1,
        "a paid case is publicly accepted"
    );
    market.moderator.restart(&market.chain.clone(), true);
    let moderator = market.moderator.node.clone();
    assert_eq!(
        case(&moderator)["fee"]["state"],
        "paid",
        "the payment survives a restart"
    );
    moderator.moderator_fee_steps();
    assert_eq!(
        last_request(&moderator, &filed),
        Some(asked),
        "a paid case is not asked again"
    );

    let (status, value) = act(&moderator, &filed, "rule", "buyer");
    assert!(status.contains("200"), "{status} {value}");
    market.deliver();
    assert_eq!(order(&market.buyer.node, &id)["status"], SETTLED);
    let (_, outpoint) = funding_tx(&market, &escrow);
    let outcome = market.chain.broadcasts().last().unwrap().clone();
    assert!(spends(&outcome, outpoint));
    assert_eq!(
        outcome.output.len(),
        2,
        "the winner and the anchor, no moderator output"
    );
    let funding = escrow["fundingSats"].as_u64().unwrap();
    assert_eq!(
        outcome.output[0].value.to_sat(),
        funding - escrow["spendFeeSats"].as_u64().unwrap() - ANCHOR_VALUE.to_sat()
    );

    let decisions = moderator.dispute_decisions_value();
    assert_eq!(decisions.len(), 1);
    let (decision, signed) = read_dispute_decision(
        &unhex(decisions[0].as_str().unwrap()).unwrap(),
        &terms.moderator.signing_key,
    )
    .unwrap();
    assert_eq!(
        (decision, json!(hex(&signed.case_id))),
        (Decision::Ruled(Party::Buyer), filed["caseId"].clone())
    );
    let (status, _) = act(&moderator, &filed, "deny", "buyer");
    assert!(status.contains("409"), "a ruled letter is not denied");
}

#[test]
fn a_denied_letter_is_final_and_its_fee_is_kept() {
    let market = Market::new();
    let (id, escrow) = funded_with(&market, FEE);
    let moderator = market.moderator.node.clone();
    let filed = filed(&market, &id);
    moderator.moderator_fee_steps();
    pay(&market, &last_request(&moderator, &filed).unwrap());

    let (status, value) = act(&moderator, &filed, "deny", "buyer");
    assert!(status.contains("200") && value["denied"] == true, "{status} {value}");
    let denied = case(&moderator);
    assert_eq!(
        (&denied["denied"], &denied["ruled"], &denied["fee"]["state"]),
        (&json!(true), &json!(false), &json!("paid")),
        "{denied}"
    );
    let (status, refused) = act(&moderator, &filed, "rule", "buyer");
    assert!(
        status.contains("409") && refused["error"].as_str().unwrap().contains("final"),
        "{status} {refused}"
    );
    assert!(act(&moderator, &filed, "deny", "buyer").0.contains("409"));
    assert!(
        act(&moderator, &filed, "deny", "vendor").0.contains("400"),
        "the vendor wrote no letter"
    );

    let decisions = moderator.dispute_decisions_value();
    let key = terms(&escrow).moderator.signing_key;
    let (decision, _) = read_dispute_decision(&unhex(decisions[0].as_str().unwrap()).unwrap(), &key).unwrap();
    assert_eq!(decision, Decision::Denied(Party::Buyer));
    assert_ne!(
        order(&market.buyer.node, &id)["status"],
        SETTLED,
        "a denial decrypts nothing"
    );
    market.deliver();
    assert_eq!(
        order(&market.buyer.node, &id)["escrow"]["disputeFee"]["state"],
        "denied"
    );
    let (status, refused) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/dispute"),
        r#"{"memo": "Again."}"#,
    );
    assert!(
        status.contains("409") && refused["error"].as_str().unwrap().contains("final"),
        "{status} {refused}"
    );
}

/// Nothing refunds a fee the moderator sits on: the paid case past its due
/// height is flagged on the desk and counted in the public track record.
#[test]
fn a_paid_case_left_undecided_past_its_due_height_is_flagged_overdue() {
    let market = Market::new();
    let (id, escrow) = funded_with(&market, FEE);
    let moderator = market.moderator.node.clone();
    let filed = filed(&market, &id);
    moderator.moderator_fee_steps();
    pay(&market, &last_request(&moderator, &filed).unwrap());
    let counts = || request(&moderator, "GET", "/moderator/profile", "").1["trackRecord"]["cases"].clone();
    assert_eq!(case(&moderator)["fee"]["overdue"], false);
    assert_eq!(counts(), json!({ "accepted": 1, "decided": 0, "denied": 0, "recused": 0, "overdue": 0 }));

    *market.chain.0.tip.lock().unwrap() = terms(&escrow).vendor_claim_height + 1;
    assert_eq!(case(&moderator)["fee"]["overdue"], true, "{}", case(&moderator));
    assert_eq!(counts(), json!({ "accepted": 1, "decided": 0, "denied": 0, "recused": 0, "overdue": 1 }));
}

#[test]
fn a_locked_moderator_asks_for_the_fee_once_unlocked() {
    let mut market = Market::new();
    let (id, _) = funded_with(&market, FEE);
    market.moderator.restart(&market.chain.clone(), false);
    let filed = filed(&market, &id);
    assert_eq!(
        filed["fee"]["state"], "awaiting-request",
        "a locked moderator still reviews the letter: {filed}"
    );
    market.moderator.node.moderator_fee_steps();
    assert_eq!(
        last_request(&market.moderator.node, &filed),
        None,
        "a request is signed with the ruling key"
    );
    unlock(&market.moderator.node);
    market.moderator.node.moderator_fee_steps();
    assert!(last_request(&market.moderator.node, &filed).is_some());
}

#[test]
fn a_moderator_with_no_filing_fee_decides_at_once() {
    let market = Market::new();
    let (id, escrow) = funded_with(&market, 0);
    assert_eq!(terms(&escrow).moderator.filing_fee().map(|fee| fee.to_sat()), Some(0));
    let moderator = market.moderator.node.clone();
    let filed = filed(&market, &id);
    assert_eq!(filed["fee"]["state"], "free", "{filed}");
    moderator.moderator_fee_steps();
    assert_eq!(last_request(&moderator, &filed), None, "nothing to pay, nothing asked");
    let (status, value) = act(&moderator, &filed, "rule", "buyer");
    assert!(status.contains("200"), "{status} {value}");
    market.deliver();
    assert_eq!(order(&market.buyer.node, &id)["status"], SETTLED);
    assert_eq!(
        moderator.dispute_decisions_value().len(),
        1,
        "a free v5 case still signs its decision"
    );
}

/// Quote the fee at `base` (`/orders/{id}` or `/escrow/sales/{id}`), then pay
/// the quote with the passphrase. Returns the quote and the answer.
pub(crate) fn quote_and_pay(node: &Node, base: &str) -> (Value, (String, Value)) {
    let (status, quote) = request(node, "POST", &format!("{base}/dispute-fee/quote"), "");
    assert!(status.contains("200"), "{status} {quote}");
    let body = json!({ "quote": quote["quote"], "passphrase": PASSPHRASE, "confirm": true }).to_string();
    (quote, request(node, "POST", &format!("{base}/dispute-fee"), &body))
}

/// The moderator's wallet sees the newest broadcast, confirmed.
fn moderator_sees_payment(market: &Market) {
    let tx = market.chain.broadcasts().last().unwrap().clone();
    market.chain.0.wallet_txs.lock().unwrap().push((tx.clone(), true));
    market.chain.confirm_next(tx.compute_txid());
    market.mine();
    market.moderator.node.moderator_fee_steps();
    market.deliver();
}

#[test]
fn the_buyer_pays_the_fee_it_was_asked_for_once_and_keeps_the_receipt() {
    let market = Market::new();
    let (id, _) = funded_with(&market, FEE);
    let (buyer, moderator) = (market.buyer.node.clone(), market.moderator.node.clone());
    let base = format!("/orders/{id}");
    let filed = filed(&market, &id);
    let (status, early) = request(&buyer, "POST", &format!("{base}/dispute-fee/quote"), "");
    assert!(
        status.contains("409"),
        "nothing to pay before the moderator asks: {status} {early}"
    );
    moderator.moderator_fee_steps();
    market.deliver();
    let asked = order(&buyer, &id)["escrow"]["disputeFee"].clone();
    assert_eq!(
        (&asked["state"], &asked["amountSats"], &asked["lightning"]),
        (&json!("requested"), &json!(FEE), &json!(false)),
        "{asked}"
    );

    fund_node_wallet(&buyer, &market.chain, 50_000);
    let (quote, (status, paid)) = quote_and_pay(&buyer, &base);
    let script = last_request(&moderator, &filed).unwrap().script;
    let address = Address::from_script(Script::from_bytes(&script), Network::Testnet4)
        .unwrap()
        .to_string();
    assert_eq!(
        (&quote["rail"], &quote["amountSats"], &quote["address"]),
        (&json!("onchain"), &json!(FEE), &json!(address)),
        "{quote}"
    );
    assert!(status.contains("200"), "{status} {paid}");
    let tx = market.chain.broadcasts().last().unwrap().clone();
    assert_eq!(paid["txid"], json!(tx.compute_txid().to_string()));
    assert!(tx
        .output
        .iter()
        .any(|out| out.script_pubkey.as_bytes() == script.as_slice() && out.value.to_sat() == FEE));
    let body = json!({ "quote": quote["quote"], "passphrase": PASSPHRASE, "confirm": true }).to_string();
    assert!(
        request(&buyer, "POST", &format!("{base}/dispute-fee"), &body)
            .0
            .contains("409"),
        "a quote pays once"
    );
    let (status, again) = request(&buyer, "POST", &format!("{base}/dispute-fee/quote"), "");
    assert!(
        status.contains("409"),
        "a paid fee is not quoted again: {status} {again}"
    );
    assert_eq!(order(&buyer, &id)["escrow"]["disputeFee"]["state"], "paying");

    moderator_sees_payment(&market);
    let receipt = order(&buyer, &id)["escrow"]["disputeFee"].clone();
    assert_eq!(
        receipt["state"], "paid",
        "the moderator's signed receipt arrived: {receipt}"
    );
    assert_eq!(case(&moderator)["fee"]["paidBy"], "buyer");
    assert!(act(&moderator, &filed, "rule", "buyer").0.contains("200"));
    market.deliver();
    assert_eq!(order(&buyer, &id)["status"], SETTLED);
}

#[test]
fn a_locked_vendor_keeps_the_fee_request_and_pays_it_once_unlocked() {
    let mut market = Market::new();
    let (id, _) = funded_with(&market, FEE);
    let sale_id = sale(&market.vendor.node)["id"].as_str().unwrap().to_string();
    let base = format!("/escrow/sales/{sale_id}");
    let (status, body) = request(
        &market.vendor.node,
        "POST",
        &format!("{base}/dispute"),
        r#"{"memo": "Delivered; signed for."}"#,
    );
    assert!(status.contains("200"), "{status} {body}");
    market.deliver();
    let filed = case(&market.moderator.node);
    assert_eq!(filed["favors"], "vendor");

    market.vendor.restart(&market.chain.clone(), false);
    market.moderator.node.moderator_fee_steps();
    market.deliver();
    assert_eq!(
        sale(&market.vendor.node)["disputeFee"]["state"],
        "requested",
        "a locked vendor still takes the request"
    );
    assert!(
        request(&market.vendor.node, "POST", &format!("{base}/dispute-fee/quote"), "")
            .0
            .contains("423")
    );
    unlock(&market.vendor.node);
    fund_node_wallet(&market.vendor.node, &market.chain, 50_000);
    let (_, (status, paid)) = quote_and_pay(&market.vendor.node, &base);
    assert!(status.contains("200"), "{status} {paid}");
    moderator_sees_payment(&market);
    assert_eq!(sale(&market.vendor.node)["disputeFee"]["state"], "paid");

    // The buyer answers with its own letter: the escrow's fee is paid, so nobody asks it.
    let (status, _) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/dispute"),
        r#"{"memo": "Empty box."}"#,
    );
    assert!(status.contains("200"));
    market.deliver();
    market.moderator.node.moderator_fee_steps();
    market.deliver();
    assert_eq!(
        order(&market.buyer.node, &id)["escrow"]["disputeFee"]["state"],
        "awaiting-request"
    );
    assert!(act(&market.moderator.node, &filed, "rule", "vendor").0.contains("200"));
    market.deliver();
    assert_eq!(sale(&market.vendor.node)["stage"], "settled");
}

#[test]
fn a_fee_the_moderators_channels_can_take_is_paid_over_lightning() {
    use openbazaar_lightning::ldk_node::bitcoin::Network as LnNetwork;
    use openbazaar_lightning::mock::MockRail;
    use std::sync::Arc;

    let market = Market::new();
    let moderator_rail = Arc::new(MockRail::new(LnNetwork::Signet));
    let buyer_rail = Arc::new(moderator_rail.linked());
    moderator_rail.set_inbound_sats(FEE);
    buyer_rail.set_lightning_sats(100_000);
    crate::lightning::sale_tests::arm(&market.moderator.node, &moderator_rail);
    crate::lightning::sale_tests::arm(&market.buyer.node, &buyer_rail);
    let (id, _) = funded_with(&market, FEE);
    let filed = filed(&market, &id);
    market.moderator.node.moderator_fee_steps();
    market.deliver();
    assert!(last_request(&market.moderator.node, &filed).unwrap().invoice.is_some(), "room in a channel: an invoice");
    assert_eq!(order(&market.buyer.node, &id)["escrow"]["disputeFee"]["lightning"], true);

    let (quote, (status, paid)) = quote_and_pay(&market.buyer.node, &format!("/orders/{id}"));
    assert_eq!(quote["rail"], "ln", "{quote}");
    assert!(status.contains("200"), "{status} {paid}");
    market.moderator.node.moderator_fee_steps();
    market.deliver();
    assert_eq!(case(&market.moderator.node)["fee"]["rail"], "ln");
    assert_eq!(order(&market.buyer.node, &id)["escrow"]["disputeFee"]["state"], "paid");
}
