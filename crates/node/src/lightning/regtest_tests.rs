//! The node's own LDK node on a local regtest chain: a demo store with no
//! channel takes a Lightning order while the wallet is locked, a buyer pays
//! it through the LSP's just-in-time channel, and the order turns Paid.
//!
//! Skips (passes with a note) when no `bitcoind` is installed; set
//! `BITCOIND_EXE` to point at one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use openbazaar_lightning::regtest::{config, wait, Bitcoind, Lsp};
use openbazaar_lightning::{ChainSource, LdkRail, LightningRail};
use serde_json::json;

use crate::harness::{hash_of, open_node, order, request, ship_to, unlock, unreachable_escrow, MockChain, TempDir, PASSPHRASE};
use crate::network::NetworkMode;

use super::LnFlags;

const LONG: Duration = Duration::from_secs(120);

#[test]
fn a_locked_node_is_paid_over_a_just_in_time_channel_and_marks_the_order_paid() {
    let Some(bitcoind) = Bitcoind::start() else {
        eprintln!("skipped: no bitcoind (set BITCOIND_EXE)");
        return;
    };
    let dirs = tempfile::tempdir().unwrap();
    let lsp = Lsp::start(&bitcoind, &dirs.path().join("lsp"));
    let buyer = LdkRail::start(config(&bitcoind, &dirs.path().join("buyer"), None), &[22u8; 32]).unwrap();
    open_funded_channel(&buyer, &bitcoind, &lsp);

    let ChainSource::BitcoindRpc { port, .. } = bitcoind.chain() else { unreachable!() };
    let args = [
        "--lightning".to_string(),
        "--ln-network".into(),
        "regtest".into(),
        "--ln-chain".into(),
        format!("bitcoind:ob:regtest@127.0.0.1:{port}"),
        "--lsp".into(),
        lsp.peer.to_string(),
        "--ln-p2p-gossip".into(),
    ];
    let mut flags = LnFlags::default();
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        flags.take(flag, |_| Ok(iter.next().cloned().unwrap_or_default())).unwrap().unwrap();
    }
    let dir = TempDir::new();
    let btc = MockChain::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &btc, 1, unreachable_escrow());
    node.enable_lightning(flags.plan(NetworkMode::Testnet4, None).unwrap().unwrap()).unwrap();

    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| node.run_lightning(&stop));
        let _stop = StopOnDrop(&stop);
        wait("the node's Lightning node", LONG, || node.ln.rail().is_some());
        let rail = node.ln.rail().unwrap().clone();
        assert!(rail.channels().is_empty());
        assert_eq!(rail.balances().onchain_total_sats, 0);
        let body = json!({"seller": "Marlow Goods", "accept": true, "acknowledgeLsp": true}).to_string();
        assert!(request(&node, "PUT", "/ln/stores", &body).0.contains("200"));

        node.lock.lock();
        let package = json!({"settlement": "direct", "rail": "ln", "lines": [{"listingHash": hash_of(&node, "Paperback novel", "Marlow Goods"), "quantity": 1}]});
        let (status, placed) = request(&node, "POST", "/checkout", &json!({"shipTo": ship_to(), "packages": [package]}).to_string());
        assert!(status.contains("200"), "{status} {placed}");
        let id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();
        let payment = placed["orders"][0]["payment"].clone();
        assert_eq!(payment["jit"], true, "{payment}");

        buyer.pay(payment["invoice"].as_str().unwrap(), 1_000_000).unwrap();
        wait("the order to be paid", LONG, || order(&node, &id)["status"] == "Paid");
        let paid = order(&node, &id);
        let amount = payment["amountMsat"].as_u64().unwrap();
        let fee = amount * u64::from(lsp.fee_ppm) / 1_000_000;
        assert_eq!(paid["payment"]["lspFeeMsat"], fee, "{paid}");
        assert_eq!(paid["payment"]["receivedMsat"], amount - fee);
        assert_eq!(rail.channels().len(), 1);
        println!("order {id} paid over Lightning: payment hash {}, LSP kept {fee} msat", payment["paymentHash"]);

        let (status, _) = request(&node, "POST", &format!("/ln/orders/{id}/pay/quote"), "");
        assert!(status.contains("423"), "paying needs the unlocked wallet");
        let channel = rail.channels()[0].id.clone();
        let close = json!({"passphrase": PASSPHRASE}).to_string();
        let (status, _) = request(&node, "POST", &format!("/ln/channels/{channel}/close"), &close);
        assert!(status.contains("423"), "closing needs the unlocked wallet");
        unlock(&node);
        let (status, value) = request(&node, "POST", "/ln/channels", &json!({"peer": lsp.peer.to_string(), "sats": 50_000, "passphrase": "wrong"}).to_string());
        assert!(status.contains("400"), "{status} {value}");
        assert_eq!(rail.channels().len(), 1);
    });
    buyer.stop();
}

/// Ends the Lightning thread when the test body returns or panics.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn open_funded_channel(buyer: &LdkRail, bitcoind: &Bitcoind, lsp: &Lsp) {
    bitcoind.send(&buyer.onchain_address().unwrap(), 0.05);
    bitcoind.mine(6);
    wait("the buyer's funds", LONG, || {
        let _ = buyer.node().sync_wallets();
        buyer.balances().onchain_spendable_sats > 0
    });
    buyer.open_channel(&lsp.peer, 2_000_000).unwrap();
    wait("the funding transaction", LONG, || bitcoind.rpc("getrawmempool", "[]").is_ok_and(|pool| pool.as_array().is_some_and(|txs| !txs.is_empty())));
    bitcoind.mine(6);
    wait("the buyer's channel", LONG, || {
        let _ = buyer.node().sync_wallets();
        let _ = lsp.node.sync_wallets();
        buyer.channels().iter().any(|channel| channel.usable)
    });
}
