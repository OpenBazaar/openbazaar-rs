//! Moderators with their own identity, on the three-node harness of
//! [`crate::multinode_tests`]: the keys come from the moderator's wallet, the
//! store chooses which moderators it accepts, the buyer picks one of them,
//! and that moderator rules.

use openbazaar_escrow::{SignedTerms, ANCHOR_VALUE};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::harness::{fund_node_wallet, hash_of, order, request, ship_to, unlock, Market, Peer};
use crate::multinode_tests::{funded, funding_tx, sale, script_of, spends};
use crate::{EscrowConfig, Node, AWAITING_FUNDING, AWAITING_TERMS, MODERATOR_FEE_SATS, SETTLED};

const SELLER: &str = "Marlow Goods";
const NAME: &str = "Ines Varga";
const FEE: u64 = 1_500;

fn profile(node: &Node) -> Value {
    let (status, value) = request(node, "GET", "/moderator/profile", "");
    assert!(status.contains("200"), "{status} {value}");
    value
}

/// The moderator's owner names it and sets its fee. Returns its id.
fn publish(market: &Market) -> String {
    let body = json!({ "name": NAME, "description": "Rules within two days.", "feeSats": FEE, "languages": ["en", "pt-br"] });
    let (status, value) = request(&market.moderator.node, "PUT", "/moderator/profile", &body.to_string());
    assert!(status.contains("200"), "{status} {value}");
    assert_eq!(value["published"], true, "{value}");
    value["id"].as_str().unwrap().to_string()
}

/// `node` accepts the moderator at `address`. Returns the answer.
fn accept(node: &Node, address: &str) -> (String, Value) {
    request(node, "POST", "/moderators", &json!({ "address": address }).to_string())
}

fn listed<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["moderators"].as_array().unwrap().iter().find(|entry| entry["id"] == id).unwrap_or(&Value::Null)
}

/// One escrow checkout of the paperback, with `moderator` if one is chosen.
fn place(node: &Node, moderator: Option<&str>) -> (String, Value) {
    let mut package = json!({ "settlement": "escrow", "lines": [{ "listingHash": hash_of(node, "Paperback novel", SELLER), "quantity": 1 }] });
    if let Some(id) = moderator {
        package["moderator"] = json!(id);
    }
    request(node, "POST", "/checkout", &json!({ "shipTo": ship_to(), "packages": [package] }).to_string())
}

fn builtin(node: &Node) -> String {
    hex(&node.builtin_moderator().unwrap())
}

fn cases(market: &Market) -> Vec<Value> {
    request(&market.moderator.node, "GET", "/moderator/disputes", "").1["disputes"].as_array().unwrap().clone()
}

#[test]
fn a_moderators_keys_come_from_its_own_wallet_and_survive_a_restart() {
    let mut market = Market::new();
    let chain = market.chain.clone();
    let other = Peer::spawn(&chain, |url| EscrowConfig { mailbox_url: Some(url), moderates: true, ..EscrowConfig::default() });
    let fresh = profile(&market.moderator.node);
    let id = fresh["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 64, "{fresh}");
    assert_ne!(profile(&other.node)["id"], json!(id), "two nodes never share a ruling key");
    assert_ne!(fresh["builtIn"]["id"], json!(id), "the node's own key is not the public test key");
    assert_eq!(fresh["published"], false, "nothing is signed before the owner names the moderator");
    let (status, refused) = accept(&market.vendor.node, &market.moderator.url);
    assert!(status.contains("400") && refused["error"].as_str().unwrap().contains("not published"), "{status} {refused}");
    assert!(request(&market.buyer.node, "GET", "/moderator/profile", "").0.contains("404"), "a node that does not moderate has no desk");

    let edit = |body: Value| request(&market.moderator.node, "PUT", "/moderator/profile", &body.to_string());
    for bad in [json!({ "name": "  " }), json!({ "name": NAME, "feeSats": 1 }), json!({ "name": NAME, "bond": 5 }), json!({ "name": NAME, "languages": ["en_US"] })] {
        let (status, value) = edit(bad.clone());
        assert!(status.contains("400"), "{bad}: {status} {value}");
    }
    assert_eq!(profile(&market.moderator.node)["published"], false, "a refused edit signs nothing");
    assert_eq!(publish(&market), id);

    market.moderator.restart(&chain, false);
    let locked = profile(&market.moderator.node);
    assert_eq!((&locked["id"], &locked["name"], &locked["feeSats"]), (&json!(id), &json!(NAME), &json!(FEE)), "{locked}");
    let (status, value) = request(&market.moderator.node, "PUT", "/moderator/profile", r#"{"name": "Someone else"}"#);
    assert!(status.contains("423"), "changing the profile signs, so it needs the wallet: {status} {value}");
    assert!(request(&market.moderator.node, "GET", "/public/moderator", "").0.contains("403"), "a browser page cannot read it");
    let (status, list) = accept(&market.vendor.node, &market.moderator.url);
    assert!(status.contains("200"), "a locked moderator still serves its signed profile: {status} {list}");
    let entry = listed(&list, &id);
    assert_eq!((&entry["name"], &entry["feeSats"], &entry["accepted"], &entry["builtIn"]), (&json!(NAME), &json!(FEE), &json!(true), &json!(false)), "{list}");
    assert_eq!(entry["trackRecord"]["rulings"], 0, "{entry}");
    assert_eq!(listed(&list, &builtin(&market.vendor.node))["trackRecord"], Value::Null, "nothing is counted for a public key");

    market.moderator.restart(&chain, true);
    assert_eq!(profile(&market.moderator.node)["id"], json!(id));
    let (status, value) = accept(&market.moderator.node, &market.moderator.url);
    assert!(status.contains("400"), "a store cannot use its own node as its moderator: {status} {value}");
}

#[test]
fn an_escrow_with_the_moderator_the_buyer_chose_is_disputed_and_ruled_by_it() {
    let mut market = Market::new();
    let chain = market.chain.clone();
    fund_node_wallet(&market.buyer.node, &chain, 100_000);
    let chosen = publish(&market);
    for node in [&market.vendor.node, &market.buyer.node] {
        let (status, value) = accept(node, &market.moderator.url);
        assert!(status.contains("200"), "{status} {value}");
    }
    let buyer = &market.buyer.node;
    let store = buyer.seller_store(SELLER).unwrap();
    assert!(store.details.as_ref().unwrap().moderators.contains(&unhex(&chosen).unwrap().try_into().unwrap()), "the accepted list is signed into the store record");
    let (_, offered) = request(buyer, "GET", &format!("/stores/{}/moderators", hex(store.id.as_bytes())), "");
    assert_eq!(offered["moderators"].as_array().unwrap().len(), 2, "{offered}");
    assert_eq!(listed(&offered, &chosen)["name"], NAME);

    let (status, placed) = place(buyer, Some(&chosen));
    assert!(status.contains("200"), "{status} {placed}");
    let placed = &placed["orders"][0];
    let id = placed["orderId"].as_str().unwrap().to_string();
    assert_eq!(placed["status"], AWAITING_TERMS);
    assert_eq!(placed["escrow"]["moderator"]["id"], json!(chosen), "{placed}");
    assert_eq!(placed["escrow"]["moderator"]["name"], NAME);
    assert_eq!(placed["escrow"]["moderatorFeeSats"], FEE);
    assert_eq!(market.deliver(), 2, "request to the vendor, offer back to the buyer");
    assert_eq!(order(buyer, &id)["status"], AWAITING_FUNDING);
    let escrow = funded(&market, &id)["escrow"].clone();
    market.mine();
    let terms = SignedTerms::decode(&unhex(escrow["terms"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(hex(&terms.terms().moderator.id()), chosen, "both parties signed the chosen moderator's record");
    assert_eq!(terms.terms().moderator.fee.to_sat(), FEE);

    // The moderator's wallet is locked: the letter is still taken and checked.
    market.moderator.restart(&chain, false);
    let (status, body) = request(&market.buyer.node, "POST", &format!("/orders/{id}/dispute"), r#"{"memo": "The box arrived empty."}"#);
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(market.deliver(), 1, "the chosen moderator reviews the letter");
    let case = cases(&market)[0].clone();
    assert_eq!((&case["favors"], &case["builtIn"], &case["ruled"]), (&json!("buyer"), &json!(false), &json!(false)), "{case}");
    let rule = format!("/moderator/disputes/{}/rule", case["id"].as_str().unwrap());
    let (status, value) = request(&market.moderator.node, "POST", &rule, r#"{"favor": "buyer"}"#);
    assert!(status.contains("423"), "a ruling is signed with the wallet's key: {status} {value}");
    unlock(&market.moderator.node);
    let (status, value) = request(&market.moderator.node, "POST", &rule, r#"{"favor": "buyer"}"#);
    assert!(status.contains("200"), "{status} {value}");
    assert_eq!(market.deliver(), 1, "the buyer settles with the ruling");

    let settled = order(&market.buyer.node, &id);
    assert_eq!(settled["status"], SETTLED, "{settled}");
    let (_, outpoint) = funding_tx(&market, &escrow);
    let outcome = market.chain.broadcasts().last().unwrap().clone();
    assert!(spends(&outcome, outpoint));
    let funding = escrow["fundingSats"].as_u64().unwrap();
    assert_eq!(outcome.output[0].script_pubkey, script_of(&escrow["buyerPayout"]));
    assert_eq!(outcome.output[0].value.to_sat(), funding - escrow["spendFeeSats"].as_u64().unwrap() - ANCHOR_VALUE.to_sat() - FEE);
    assert_eq!(outcome.output[1].value.to_sat(), FEE, "the fee in the moderator's own record, not the built-in one");
    assert_eq!(outcome.output[1].script_pubkey, terms.terms().moderator.payout);
    assert_ne!(FEE, MODERATOR_FEE_SATS);

    // A demo listing has no stable id, so this ruling cites no listing and is
    // not public: the count stays at what can be verified.
    assert_eq!(cases(&market)[0]["ruled"], true);
    assert_eq!(profile(&market.moderator.node)["trackRecord"], json!({ "rulings": 0, "stores": 0 }));
    let (_, list) = accept(&market.vendor.node, &market.moderator.url);
    assert_eq!(listed(&list, &chosen)["trackRecord"]["rulings"], 0, "{list}");
    assert_eq!(sale(&market.vendor.node)["stage"], "funded", "the buyer disputed; the vendor's sale waits for the outcome");
}

#[test]
fn a_buyer_cannot_use_a_moderator_the_store_did_not_accept() {
    let market = Market::new();
    let (buyer, vendor) = (&market.buyer.node, &market.vendor.node);
    let chosen = publish(&market);
    let key: [u8; 32] = unhex(&chosen).unwrap().try_into().unwrap();
    let refused = |moderator: Option<&str>, why: &str| {
        let (status, value) = place(buyer, moderator);
        assert!(status.contains("400") && value["error"].as_str().unwrap().contains(why), "{moderator:?}: {status} {value}");
    };

    refused(Some(&chosen), "signed record does not accept");
    refused(Some("zz"), "not a moderator's key");
    assert!(accept(buyer, &market.moderator.url).0.contains("200"));
    refused(Some(&chosen), "seller's node does not accept");
    assert!(vendor.sale_moderator(SELLER, Some(key)).is_err(), "the vendor refuses a request that names it anyway");
    assert!(vendor.sale_moderator(SELLER, None).is_ok(), "the built-in moderator is still accepted");

    // The store stops accepting the built-in moderator: the default request is refused too.
    assert!(accept(vendor, &market.moderator.url).0.contains("200"));
    let (status, list) = request(vendor, "DELETE", &format!("/moderators/{}", builtin(vendor)), "");
    assert!(status.contains("200"), "{status} {list}");
    assert_eq!(listed(&list, &builtin(vendor))["accepted"], false, "{list}");
    refused(None, "seller's node does not accept");
    assert!(vendor.sale_moderator(SELLER, None).is_err());
    assert!(vendor.sale_moderator(SELLER, Some(key)).is_ok());
    let (_, orders) = request(buyer, "GET", "/orders", "");
    assert_eq!(orders["orders"].as_array().map_or(0, Vec::len), 0, "a refused checkout places nothing: {orders}");

    // With no moderator accepted, the store does not offer moderated escrow.
    let (status, list) = request(vendor, "DELETE", &format!("/moderators/{chosen}"), "");
    assert!(status.contains("200"), "{status} {list}");
    let (_, catalog) = request(vendor, "GET", "/catalog", "");
    assert!(catalog["listings"].as_array().unwrap().iter().all(|listing| listing["acceptsModerator"] == false), "{catalog}");
    let (status, value) = place(vendor, None);
    assert!(status.contains("400"), "{status} {value}");
}
