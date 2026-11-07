//! Funding and release survive restarts; coins stay locked while an open
//! waits; the outbox retries and repeated mail is handled once.

use crate::coin_locks::OPEN_LOCK_TTL;
use crate::escrow_outbox::now_secs;
use crate::harness::{checkout, fund_node_wallet, order, pay, request, unlock, Market};
use crate::multinode_tests::{funding_tx, offered, sale, spends};
use crate::{AWAITING_FUNDING, FUNDED, RELEASED};

use super::{deliver_restarting, stage};

const SELLER: &str = "Marlow Goods";

#[test]
fn every_node_restarts_before_every_round_and_the_escrow_still_funds_and_releases() {
    let mut market = Market::new();
    fund_node_wallet(&market.buyer.node, &market.chain, 100_000);
    let placed = checkout(&market.buyer.node, &[("Paperback novel", SELLER, "escrow")]);
    let id = placed[0]["orderId"].as_str().unwrap().to_string();
    let rounds = deliver_restarting(&mut market, |_| false);
    assert_eq!(rounds, 2, "request and offer");
    assert_eq!(stage(&market, &id), "offered");
    assert_eq!(sale(&market.vendor.node)["stage"], "offered");

    let (status, opened) = pay(&market.buyer.node, &id, "{}");
    assert!(status.contains("202"), "{status} {opened}");
    // The buyer loses its round 1 nonces once here and opens again. After
    // that, restarting it while it waits for the reply would only repeat that.
    let rounds = deliver_restarting(&mut market, |market| {
        let escrow = order(&market.buyer.node, &id)["escrow"].clone();
        escrow["stage"] == "opened" && escrow["openAttempt"].as_u64() >= Some(2)
    });
    assert!(rounds >= 5, "open, reply, open again, reply, signatures, nonce: {rounds}");
    let after = order(&market.buyer.node, &id);
    assert_eq!(after["escrow"]["stage"], "funded", "{after}");
    assert_eq!(after["escrow"]["openAttempt"], 2);
    let sold = sale(&market.vendor.node);
    assert_eq!(sold["stage"], "funded");
    assert_eq!(sold["claimHeld"], true, "the claim is on disk");
    let (_, outpoint) = funding_tx(&market, &after["escrow"]);
    assert_eq!(
        market.chain.broadcasts().iter().filter(|tx| tx.compute_txid() == outpoint.txid).count(),
        1,
        "one funding"
    );

    market.restart_all();
    market.mine();
    assert_eq!(order(&market.buyer.node, &id)["status"], FUNDED);
    let (status, released) = request(&market.buyer.node, "POST", &format!("/orders/{id}/release"), "{}");
    assert!(status.contains("200"), "{status} {released}");
    // The vendor's release nonce is gone: it sends a fresh one and the buyer signs again.
    market.restart_all();
    assert_eq!(market.deliver(), 3, "release, fresh nonce, release again");
    let release = market.chain.broadcasts().last().unwrap().clone();
    assert!(spends(&release, outpoint));
    assert_eq!(sale(&market.vendor.node)["stage"], "released");
    assert_eq!(order(&market.buyer.node, &id)["status"], RELEASED);
}

#[test]
fn a_locked_vendor_keeps_the_open_until_it_is_unlocked() {
    let mut market = Market::new();
    fund_node_wallet(&market.buyer.node, &market.chain, 100_000);
    let (id, _) = offered(&market);
    let chain = market.chain.clone();
    market.vendor.restart(&chain, false);
    let (status, opened) = pay(&market.buyer.node, &id, "{}");
    assert!(status.contains("202"), "{status} {opened}");
    assert_eq!(market.deliver(), 0, "the vendor's round needs its per-order key");
    assert_eq!(market.vendor.node.escrow.inbox.list().unwrap().len(), 1, "the open waits");
    unlock(&market.vendor.node);
    assert_eq!(market.deliver(), 4, "open, reply, signatures, release nonce");
    assert_eq!(stage(&market, &id), "funded");
}

#[test]
fn an_open_locks_its_coins_until_it_expires() {
    let market = Market::new();
    let buyer = &market.buyer.node;
    fund_node_wallet(buyer, &market.chain, 100_000);
    let (id, _) = offered(&market);
    let (status, _) = pay(buyer, &id, "{}");
    assert!(status.contains("202"));
    assert_eq!(buyer.escrow.coins.outpoints().len(), 1, "the funding's only input");

    let placed = checkout(buyer, &[("Paperback novel", SELLER, "direct")]);
    let direct = placed[0]["orderId"].as_str().unwrap().to_string();
    let (status, refused) = pay(buyer, &direct, "{}");
    assert!(!status.contains("202"), "the locked coin is not spent twice: {status} {refused}");

    assert_eq!(buyer.expire_escrow_opens(now_secs() + 10), 0, "not yet");
    assert_eq!(buyer.expire_escrow_opens(now_secs() + OPEN_LOCK_TTL.as_secs() + 1), 1);
    let reset = order(buyer, &id);
    assert_eq!(reset["escrow"]["stage"], "offered");
    assert_eq!(reset["status"], AWAITING_FUNDING);
    assert!(buyer.escrow.coins.outpoints().is_empty());

    assert_eq!(market.deliver(), 1, "the vendor replies; the buyer drops the stale reply");
    assert_eq!(stage(&market, &id), "offered");
    let (status, paid) = pay(buyer, &direct, "{}");
    assert!(status.contains("202"), "{status} {paid}");
    assert!(paid.get("payError").is_none(), "{paid}");
}

#[test]
fn letters_wait_in_the_outbox_while_a_peer_is_down_and_repeats_are_handled_once() {
    let market = Market::new();
    market.vendor.stop();
    let placed = checkout(&market.buyer.node, &[("Paperback novel", SELLER, "escrow")]);
    let id = placed[0]["orderId"].as_str().unwrap().to_string();
    let waiting = order(&market.buyer.node, &id);
    assert!(waiting["escrow"]["error"].as_str().unwrap().contains("keeps trying"), "{waiting}");
    assert_eq!(market.queued(), 1);
    assert_eq!(market.deliver(), 0, "the vendor answers 503");
    assert_eq!(market.queued(), 1, "still queued");

    market.vendor.start();
    market.buyer.node.flush_escrow_outbox(true);
    assert_eq!(market.queued(), 0);
    let vendor = &market.vendor.node;
    let letters = vendor.escrow.inbox.list().unwrap();
    assert_eq!(letters.len(), 1);
    let request = vendor.escrow.inbox.read(&letters[0]).unwrap().to_bytes();
    assert_eq!(market.deliver(), 2, "request, offer");
    assert_eq!(stage(&market, &id), "offered");
    assert!(order(&market.buyer.node, &id)["escrow"]["error"].is_null(), "the offer clears the error");

    // The same letter again, as when a retry crossed a lost response.
    vendor.accept_escrow_mail(&request).unwrap();
    assert_eq!(vendor.process_escrow_mail(), 0, "the replay guard drops it");
    assert!(vendor.escrow.inbox.list().unwrap().is_empty());
    assert!(market.buyer.node.escrow.inbox.list().unwrap().is_empty(), "no second offer");
    assert_eq!(market.queued(), 0);
}
