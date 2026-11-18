//! Paying a filing fee back from the moderator's wallet, on the three-node
//! harness: a recusal before any decision.

use bitcoin::ScriptBuf;
use openbazaar_escrow::{read_dispute_decision, Decision, Party};
use serde_json::{json, Value};

use crate::escrow_mail::{parse_id, unhex, EscrowId};
use crate::harness::{fund_node_wallet, order, request, Market, PASSPHRASE};
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

