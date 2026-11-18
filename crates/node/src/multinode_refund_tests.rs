//! Paying a filing fee back from the moderator's wallet, on the three-node
//! harness: a recusal before any decision, and a payment that came too late
//! for its case after the script was given to another.

use bitcoin::absolute::LockTime;
use bitcoin::{Address, Network, Script, ScriptBuf};
use openbazaar_escrow::{read_dispute_decision, Decision, Party};
use serde_json::{json, Value};

use crate::escrow_mail::{parse_id, unhex, EscrowId};
use crate::harness::{fund_node_wallet, hash_of, order, pay as pay_order, request, ship_to, Market, PASSPHRASE};
use crate::multinode_fee_tests::{act, case, filed, funded_with, last_request, pay, terms, FEE};
use crate::Node;

/// Quote at `path/quote`, then confirm at `path` with the passphrase.
pub(crate) fn quote_and_confirm(node: &Node, path: &str, body: &str) -> (Value, (String, Value)) {
    let (status, quote) = request(node, "POST", &format!("{path}/quote"), body);
    assert!(status.contains("200"), "{status} {quote}");
    let confirm = json!({ "quote": quote["quote"], "passphrase": PASSPHRASE, "confirm": true }).to_string();
    (quote, request(node, "POST", path, &confirm))
}

pub(crate) fn escrow_of(case: &Value) -> EscrowId {
    parse_id(case["id"].as_str().unwrap()).unwrap()
}

pub(crate) fn refund_script(node: &Node, party: Party, escrow: &EscrowId) -> ScriptBuf {
    ScriptBuf::from_bytes(node.filer_fee(party, escrow).unwrap().refund_script.expect("a v5 letter names a refund script"))
}

#[test]
fn a_recusal_after_payment_pays_the_fee_back_to_the_payers_refund_script() {
    let market = Market::new();
    let (id, escrow) = funded_with(&market, FEE);
    let moderator = market.moderator.node.clone();
    let filed = filed(&market, &id);
    let path = format!("/moderator/disputes/{}/recuse", filed["id"].as_str().unwrap());
    let (status, refused) = request(&moderator, "POST", &format!("{path}/quote"), "");
    assert!(
        status.contains("409") && refused["error"].as_str().unwrap().contains("Nothing was paid"),
        "{status} {refused}"
    );
    moderator.moderator_fee_steps();
    pay(&market, &last_request(&moderator, &filed).unwrap());
    fund_node_wallet(&moderator, &market.chain, 50_000);

    let script = refund_script(&market.buyer.node, Party::Buyer, &escrow_of(&filed));
    let (quote, (status, value)) = quote_and_confirm(&moderator, &path, "");
    assert!(status.contains("200"), "{status} {value}");
    assert_eq!((&quote["amountSats"], &quote["filer"]), (&json!(FEE), &json!("buyer")), "{quote}");
    let refund = market.chain.broadcasts().last().unwrap().clone();
    assert_eq!(value["txid"], json!(refund.compute_txid().to_string()));
    assert!(
        refund.output.iter().any(|out| out.script_pubkey == script && out.value.to_sat() == FEE),
        "the whole fee goes back to the script in the buyer's letter: {refund:?}"
    );
    let fee = case(&moderator)["fee"].clone();
    assert_eq!((&fee["recused"], &fee["refunds"][0]["txid"]), (&json!(true), &value["txid"]), "{fee}");

    let decisions = moderator.dispute_decisions_value();
    let key = terms(&escrow).moderator.signing_key;
    let (decision, _) = read_dispute_decision(&unhex(decisions[0].as_str().unwrap()).unwrap(), &key).unwrap();
    assert_eq!(decision, Decision::Recused, "the recusal is a public decision");
    let counts = request(&moderator, "GET", "/moderator/profile", "").1["trackRecord"]["cases"].clone();
    assert_eq!(counts, json!({ "accepted": 1, "decided": 0, "denied": 0, "recused": 1, "overdue": 0 }));

    market.deliver();
    let held = order(&market.buyer.node, &id)["escrow"]["disputeFee"].clone();
    assert_eq!((&held["state"], &held["refundTxid"]), (&json!("recused"), &value["txid"]), "{held}");

    for (action, favor) in [("rule", "buyer"), ("deny", "buyer")] {
        let (status, refused) = act(&moderator, &filed, action, favor);
        assert!(
            status.contains("409") && refused["error"].as_str().unwrap().contains("recused"),
            "{action} after a recusal: {status} {refused}"
        );
    }
    let (status, again) = request(&moderator, "POST", &format!("{path}/quote"), "");
    assert!(status.contains("409"), "one recusal, one refund: {status} {again}");
    let (status, refiled) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/dispute"),
        r#"{"memo": "Again."}"#,
    );
    assert!(status.contains("409"), "{status} {refiled}");
}

#[test]
fn a_decided_case_cannot_be_recused() {
    let market = Market::new();
    let (id, _) = funded_with(&market, FEE);
    let moderator = market.moderator.node.clone();
    let filed = filed(&market, &id);
    moderator.moderator_fee_steps();
    pay(&market, &last_request(&moderator, &filed).unwrap());
    fund_node_wallet(&moderator, &market.chain, 50_000);
    let path = format!("/moderator/disputes/{}/recuse", filed["id"].as_str().unwrap());
    let (status, quote) = request(&moderator, "POST", &format!("{path}/quote"), "");
    assert!(status.contains("200"), "{status} {quote}");
    let broadcasts = market.chain.broadcasts().len();

    let (status, ruled) = act(&moderator, &filed, "rule", "buyer");
    assert!(status.contains("200"), "{status} {ruled}");
    let (status, refused) = request(&moderator, "POST", &format!("{path}/quote"), "");
    assert!(
        status.contains("409") && refused["error"].as_str().unwrap().contains("decided"),
        "{status} {refused}"
    );
    let confirm = json!({ "quote": quote["quote"], "passphrase": PASSPHRASE, "confirm": true }).to_string();
    let (status, refused) = request(&moderator, "POST", &path, &confirm);
    assert!(
        status.contains("409") && refused["error"].as_str().unwrap().contains("decided"),
        "a quote taken before the ruling pays nothing after it: {status} {refused}"
    );
    assert_eq!(market.chain.broadcasts().len(), broadcasts, "no refund went out");
    assert_eq!(case(&moderator)["fee"]["recused"], false);
}

#[test]
fn a_refund_never_goes_to_this_nodes_own_wallet() {
    let market = Market::new();
    let (id, _) = funded_with(&market, FEE);
    let moderator = market.moderator.node.clone();
    let filed = filed(&market, &id);
    moderator.moderator_fee_steps();
    let asked = last_request(&moderator, &filed).unwrap();
    pay(&market, &asked);
    fund_node_wallet(&moderator, &market.chain, 50_000);
    moderator.rewrite_refund_script(&escrow_of(&filed), Party::Buyer, asked.script.clone());
    let path = format!("/moderator/disputes/{}/recuse/quote", filed["id"].as_str().unwrap());
    let (status, refused) = request(&moderator, "POST", &path, "");
    assert!(
        status.contains("409") && refused["error"].as_str().unwrap().contains("own wallet"),
        "a refund into one of its own fee scripts would count as a payment: {status} {refused}"
    );
}

/// A second funded order under the same moderator. Returns its id.
fn another_order(market: &Market) -> String {
    let body = json!({ "name": "Ines Varga", "feeSats": FEE });
    let chosen = request(&market.moderator.node, "PUT", "/moderator/profile", &body.to_string()).1["id"].clone();
    let buyer = &market.buyer.node;
    fund_node_wallet(buyer, &market.chain, 100_000);
    let package = json!({ "settlement": "escrow", "moderator": chosen, "lines": [{ "listingHash": hash_of(buyer, "Paperback novel", "Marlow Goods"), "quantity": 1 }] });
    let (status, placed) = request(buyer, "POST", "/checkout", &json!({ "shipTo": ship_to(), "packages": [package] }).to_string());
    assert!(status.contains("200"), "{status} {placed}");
    let id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();
    market.deliver();
    let (status, opened) = pay_order(buyer, &id, "{}");
    assert!(status.contains("202"), "{status} {opened}");
    market.deliver();
    assert_eq!(order(buyer, &id)["escrow"]["stage"], "funded");
    market.mine();
    id
}

fn case_of(node: &Node, escrow: &EscrowId) -> Value {
    let all = request(node, "GET", "/moderator/disputes", "").1["disputes"].clone();
    all.as_array().unwrap().iter().find(|case| escrow_of(case) == *escrow).unwrap().clone()
}

#[test]
fn a_late_payment_after_the_script_moved_on_is_owed_back_and_not_credited_to_the_new_case() {
    let market = Market::new();
    let moderator = market.moderator.node.clone();
    let (first_id, first_escrow) = funded_with(&market, FEE);
    let first = escrow_of(&filed(&market, &first_id));
    moderator.moderator_fee_steps();
    let asked = last_request(&moderator, &case_of(&moderator, &first)).unwrap();
    let asked_at = market.chain.tip();
    *market.chain.0.tip.lock().unwrap() = terms(&first_escrow).vendor_claim_height + 7;
    moderator.moderator_fee_steps();

    let second_id = another_order(&market);
    let (status, body) = request(&market.buyer.node, "POST", &format!("/orders/{second_id}/dispute"), r#"{"memo": "Wrong size."}"#);
    assert!(status.contains("200"), "{status} {body}");
    market.deliver();
    let second = (request(&moderator, "GET", "/moderator/disputes", "").1["disputes"].as_array().unwrap().iter())
        .map(escrow_of)
        .find(|escrow| *escrow != first)
        .unwrap();
    moderator.moderator_fee_steps();
    let reasked = last_request(&moderator, &case_of(&moderator, &second)).unwrap();
    assert_eq!(reasked.script, asked.script, "the unpaid script went to the next case");

    let address = Address::from_script(Script::from_bytes(&asked.script), Network::Testnet4).unwrap();
    let late = market.chain.send_to_wallet_locked(&address, FEE, true, LockTime::from_height(asked_at + 1).unwrap());
    market.chain.confirm_next(late.compute_txid());
    market.mine();
    moderator.moderator_fee_steps();
    assert_eq!(case_of(&moderator, &second)["fee"]["state"], "requested", "made for the first request, it pays nothing now");
    assert_eq!(case_of(&moderator, &second)["fee"]["owed"], json!([]));
    let owed = case_of(&moderator, &first)["fee"]["owed"].clone();
    let payment = format!("{}:0", late.compute_txid());
    assert_eq!(owed, json!([{ "filer": "buyer", "amountSats": FEE, "payment": payment }]));

    fund_node_wallet(&moderator, &market.chain, 50_000);
    let path = format!("/moderator/disputes/{}/refund", crate::escrow_mail::id_hex(&first));
    let body = json!({ "payment": payment }).to_string();
    let (quote, (status, value)) = quote_and_confirm(&moderator, &path, &body);
    assert!(status.contains("200"), "{status} {value}");
    assert_eq!((&quote["amountSats"], &quote["payment"]), (&json!(FEE), &json!(payment)), "{quote}");
    let script = refund_script(&market.buyer.node, Party::Buyer, &first);
    let refund = market.chain.broadcasts().last().unwrap().clone();
    assert!(refund.output.iter().any(|out| out.script_pubkey == script && out.value.to_sat() == FEE), "{refund:?}");
    let fee = case_of(&moderator, &first)["fee"].clone();
    assert_eq!((&fee["owed"], &fee["refunds"][0]["owed"]), (&json!([]), &json!(payment)), "{fee}");
    let (status, again) = request(&moderator, "POST", &format!("{path}/quote"), &body);
    assert!(status.contains("409"), "sent back once: {status} {again}");

    pay(&market, &reasked);
    assert_eq!(case_of(&moderator, &second)["fee"]["state"], "paid", "the new case's own payment counts");
    assert_eq!(case_of(&moderator, &first)["fee"]["owed"], json!([]));
}
