//! Real LDK nodes on a local regtest chain: a seller with no channel and no
//! on-chain funds gets paid through an LSPS2 just-in-time channel.
//!
//! Skips (passes with a note) when no `bitcoind` is installed; set
//! `BITCOIND_EXE` to point at one.

use std::time::Duration;

use openbazaar_lightning::regtest::{config, wait, Bitcoind, Lsp};
use openbazaar_lightning::{decode_invoice, Incoming, LdkRail, LightningRail, Outgoing};

const LONG: Duration = Duration::from_secs(120);

#[test]
fn seller_is_paid_through_a_just_in_time_channel_and_keeps_it_across_a_restart() {
    let Some(bitcoind) = Bitcoind::start() else {
        eprintln!("skipped: no bitcoind (set BITCOIND_EXE)");
        return;
    };
    let dirs = tempfile::tempdir().unwrap();
    let lsp = Lsp::start(&bitcoind, &dirs.path().join("lsp"));

    let seller_secret = [11u8; 32];
    let seller_dir = dirs.path().join("seller");
    let seller = LdkRail::start(config(&bitcoind, &seller_dir, Some(lsp.config())), &seller_secret).unwrap();
    let buyer = LdkRail::start(config(&bitcoind, &dirs.path().join("buyer"), None), &[22u8; 32]).unwrap();

    // The buyer funds a channel to the LSP; the seller has nothing at all.
    let address = buyer.onchain_address().unwrap();
    bitcoind.send(&address, 0.05);
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
    assert_eq!(seller.balances().onchain_total_sats, 0);
    assert!(seller.channels().is_empty());

    // An order invoice: no channel has room, so it routes through the LSP.
    let amount_msat = 50_000_000;
    let invoice = seller.receive(amount_msat, "OpenBazaar order 1", 3_600).unwrap();
    assert!(invoice.jit, "{invoice:?}");
    assert_eq!(invoice.lsp_fee_cap_msat, Some(5_000_000));
    let info = decode_invoice(&invoice.bolt11, ldk_node::bitcoin::Network::Regtest).unwrap();
    assert_eq!(info.amount_msat, Some(amount_msat));
    assert_eq!(info.payee, seller.info().node_id);
    assert_eq!(seller.incoming(&invoice.payment_hash), Incoming::Pending);

    let hash = buyer.pay(&invoice.bolt11, 1_000_000).unwrap();
    assert_eq!(hash, invoice.payment_hash);
    wait("the seller to be paid", LONG, || matches!(seller.incoming(&invoice.payment_hash), Incoming::Paid { .. }));
    let Incoming::Paid { received_msat, lsp_fee_msat, .. } = seller.incoming(&invoice.payment_hash) else { unreachable!() };
    let expected_fee = amount_msat * u64::from(lsp.fee_ppm) / 1_000_000;
    assert_eq!(lsp_fee_msat, expected_fee);
    assert_eq!(received_msat, amount_msat - expected_fee);
    wait("the buyer's record", LONG, || matches!(buyer.outgoing(&hash), Outgoing::Paid { .. }));

    let channels = seller.channels();
    assert_eq!(channels.len(), 1, "{channels:?}");
    assert_eq!(channels[0].peer, lsp.peer.node_id.to_string());
    assert!(channels[0].ready);
    let lightning = seller.balances().lightning_sats;
    assert!(lightning >= (received_msat / 1_000).saturating_sub(1_000), "{lightning}");
    println!("payment hash {} paid {received_msat} msat to the seller, LSP kept {lsp_fee_msat} msat", invoice.payment_hash);

    // The sealed store brings the channel and the payment back; another key does not open it.
    let node_id = seller.info().node_id;
    seller.stop();
    drop(seller);
    assert!(LdkRail::start(config(&bitcoind, &seller_dir, Some(lsp.config())), &[12u8; 32]).is_err());
    let seller = LdkRail::start(config(&bitcoind, &seller_dir, Some(lsp.config())), &seller_secret).unwrap();
    assert_eq!(seller.info().node_id, node_id);
    assert_eq!(seller.channels().len(), 1);
    assert!(matches!(seller.incoming(&invoice.payment_hash), Incoming::Paid { .. }));

    // The channel is twice the payment: a small second payment fits it, with no LSP fee.
    wait("the seller to reconnect", LONG, || seller.channels().iter().any(|channel| channel.usable));
    let second = seller.receive(2_000_000, "OpenBazaar order 2", 3_600).unwrap();
    assert!(!second.jit);
    buyer.pay(&second.bolt11, 1_000_000).unwrap();
    wait("the second payment", LONG, || matches!(seller.incoming(&second.payment_hash), Incoming::Paid { lsp_fee_msat: 0, .. }));
    seller.stop();
    buyer.stop();
}
