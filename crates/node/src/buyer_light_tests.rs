//! End-to-end tests for buyer-only light mode:
//! 1. Deterministic 12-word BIP-39 recovery of buyer order, reply, escrow, and payout keys.
//! 2. Pull-mode remote checkout & authenticated `POST /mail/pull` (no 24/7 buyer mailbox).
//! 3. Rejection of unauthorized callers, unknown sales, and browser Origin headers on `/mail/pull`.
//! 4. Local signed contract retention allowing direct-to-moderator disputes even after the seller
//!    shuts off their node, plus detection of tampered escrow addresses.
//! 5. Mainnet escrow still strictly returning `ESCROW_MAINNET_REFUSAL`.

use serde_json::{json, Value};
use tormail::SenderKey;

use crate::buyer_light::{build_pull_request, open_pull_response, BuyerSeed, RetainedContract};
use crate::escrow_mail::{hex, unhex};
use crate::harness::{fund_node_wallet, open_node, order, pay, request, ship_to, MockChain, Peer, TempDir};
use crate::http::dispatch;
use crate::instructions::ESCROW_MAINNET_REFUSAL;
use crate::network::{NetworkMode, ServeOptions};
use crate::seller_store_tests::open_store;
use crate::watch::{AWAITING_PAYMENT, COMPLETED, FUNDED, PAID, SHIPPED};
use crate::EscrowConfig;

fn post_raw(node: &crate::Node, path: &str, body: &[u8], origin: Option<&str>) -> (String, Value) {
    let origin_hdr = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
    let raw = [
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{origin_hdr}Content-Length: {}\r\n\r\n",
            body.len()
        )
        .as_bytes(),
        body,
    ]
    .concat();
    let response = String::from_utf8_lossy(&dispatch(node, &raw)).to_string();
    let (head, body_str) = response.split_once("\r\n\r\n").unwrap();
    (
        head.lines().next().unwrap().to_string(),
        serde_json::from_str(body_str).unwrap_or(Value::Null),
    )
}

#[test]
fn pull_mode_order_works_without_buyer_mailbox_and_rejects_strangers() {
    let chain = MockChain::new();
    let seller = Peer::spawn_with(&chain, 1, |url| EscrowConfig {
        mailbox_url: Some(url),
        ..EscrowConfig::default()
    });
    let listing = open_store(&seller.node);
    let (status, _) = request(
        &seller.node,
        "PUT",
        "/store/profile",
        &json!({ "returnsPolicy": "Returns accepted within 14 days." }).to_string(),
    );
    assert!(status.contains("200"), "{status}");

    // Light buyer node with NO mailbox_url (no 24/7 Tor onion or loopback inbox).
    let buyer_dir = TempDir::new();
    let buyer = open_node(
        &buyer_dir.0,
        NetworkMode::Testnet4,
        &chain,
        1,
        EscrowConfig::default(),
    );
    assert!(buyer.sale_reply_url().is_none(), "light buyer has no push reply URL");

    let (status, follows) = request(&buyer, "POST", "/follows", &json!({ "url": seller.url }).to_string());
    assert!(status.contains("200"), "{status} {follows}");

    // Place a direct order in pull mode.
    let checkout_body = json!({
        "shipTo": ship_to(),
        "packages": [{ "settlement": "direct", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    let (status, placed) = request(&buyer, "POST", "/checkout", &checkout_body.to_string());
    assert!(status.contains("200"), "{status} {placed}");
    let order_id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();

    // Seller immediately returned sealed terms on POST /mail -> buyer already has payment address!
    let current = order(&buyer, &order_id);
    assert_eq!(current["status"], AWAITING_PAYMENT, "{current}");
    assert_eq!(current["remote"]["replyMode"], "pull");
    let pay_address = current["payment"]["address"].as_str().unwrap().to_string();
    assert!(pay_address.starts_with("tb1p"), "{pay_address}");
    let total_sats = current["totalSats"].as_u64().unwrap();
    assert_eq!(total_sats, 25_000 + 3_000);

    // Fund from an external wallet (directly onto the chain, no buyer node wallet needed!).
    let sale_id = {
        let (_, sales) = request(&seller.node, "GET", "/sales", "");
        sales["sales"][0]["id"].as_str().unwrap().to_string()
    };
    chain.pay_external(&pay_address, total_sats);
    chain.mine();
    seller.node.watch_once().unwrap();
    assert_eq!(request(&seller.node, "GET", "/sales", "").1["sales"][0]["status"], PAID);

    // Seller ships and writes a message while buyer has no mailbox to receive pushes.
    let (status, shipped) = request(
        &seller.node,
        "POST",
        &format!("/sales/{sale_id}/fulfill"),
        &json!({ "carrier": "USPS", "tracking": "9400123456789" }).to_string(),
    );
    assert!(status.contains("200"), "{status} {shipped}");
    let (status, _) = request(
        &seller.node,
        "POST",
        &format!("/sales/{sale_id}/message"),
        &json!({ "text": "Packed carefully, dropped at the post office." }).to_string(),
    );
    assert!(status.contains("200"), "{status}");

    // Unauthorized callers on POST /mail/pull MUST be refused uniformly.
    let store_record = seller.node.own_record().unwrap();
    let store_mailbox = crate::stores::mailbox_key(&store_record).unwrap();
    let drop_bytes: [u8; 16] = unhex(&sale_id).unwrap().try_into().unwrap();
    let nonce = [0xabu8; 16];

    // 1. Stranger with wrong buyer key querying the real sale id -> 403.
    let stranger_letter = build_pull_request(&drop_bytes, &SenderKey::generate(), &store_mailbox, &nonce).unwrap();
    let (stranger_status, stranger_body) = post_raw(&seller.node, "/mail/pull", &stranger_letter.to_bytes(), None);
    assert!(stranger_status.contains("403"), "{stranger_status} {stranger_body}");

    // 2. Stranger querying a nonexistent sale id -> identical 403 error (no oracle).
    let missing_letter = build_pull_request(&[0x99u8; 16], &SenderKey::generate(), &store_mailbox, &nonce).unwrap();
    let (missing_status, missing_body) = post_raw(&seller.node, "/mail/pull", &missing_letter.to_bytes(), None);
    assert!(missing_status.contains("403"), "{missing_status} {missing_body}");
    assert_eq!(stranger_body, missing_body, "stranger and missing sale errors must be indistinguishable");

    // 3. Browser request carrying Origin header -> 403 (web stays browse-only).
    let (origin_status, _) = post_raw(
        &seller.node,
        "/mail/pull",
        &stranger_letter.to_bytes(),
        Some("https://evil.example"),
    );
    assert!(origin_status.contains("403"), "{origin_status}");

    // 4. Garbage payload -> 400.
    let (bad_status, _) = post_raw(&seller.node, "/mail/pull", b"not a sealed letter", None);
    assert!(bad_status.contains("400"), "{bad_status}");

    // Legitimate buyer pulls sealed updates via POST /orders/{id}/pull.
    let (status, pulled) = request(&buyer, "POST", &format!("/orders/{order_id}/pull"), "");
    assert!(status.contains("200"), "{status} {pulled}");
    assert_eq!(pulled["status"], SHIPPED, "{pulled}");
    assert_eq!(pulled["remote"]["fulfillment"]["carrier"], "USPS");
    assert_eq!(pulled["remote"]["fulfillment"]["tracking"], "9400123456789");
    assert_eq!(
        pulled["remote"]["messages"][0]["text"],
        "Packed carefully, dropped at the post office."
    );
    assert_eq!(pulled["remote"]["returnPolicy"], "Returns accepted within 14 days.");

    // Buyer confirms arrival -> seller marks sale completed.
    let (status, done) = request(&buyer, "POST", &format!("/orders/{order_id}/complete"), "");
    assert!(status.contains("200"), "{status} {done}");
    assert_eq!(done["status"], COMPLETED);
    buyer.flush_order_mail(true);
    assert_eq!(request(&seller.node, "GET", "/sales", "").1["sales"][0]["status"], COMPLETED);
}

#[test]
fn retained_contract_allows_offline_seller_dispute_and_rejects_tampered_escrow_address() {
    let chain = MockChain::new();
    let moderator = Peer::spawn_with(&chain, 1, |url| EscrowConfig {
        mailbox_url: Some(url),
        moderates: true,
        ..EscrowConfig::default()
    });
    let mod_url = moderator.url.clone();
    let seller = Peer::spawn_with(&chain, 1, move |url| EscrowConfig {
        mailbox_url: Some(url),
        moderator_url: Some(mod_url),
        ..EscrowConfig::default()
    });
    let listing = open_store(&seller.node);
    let vendor_url = seller.url.clone();
    let mod_url = moderator.url.clone();
    let buyer = Peer::spawn_with(&chain, 1, move |url| EscrowConfig {
        mailbox_url: Some(url),
        vendors: [("Brook and Pine".to_string(), vendor_url)].into_iter().collect(),
        moderator_url: Some(mod_url),
        ..EscrowConfig::default()
    });
    let (status, _) = request(&buyer.node, "POST", "/follows", &json!({ "url": seller.url }).to_string());
    assert!(status.contains("200"), "{status}");

    let body = json!({
        "shipTo": ship_to(),
        "packages": [{ "settlement": "escrow", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    let (status, placed) = request(&buyer.node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("200"), "{status} {placed}");
    let order_id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();

    let pump = |b: &Peer, s: Option<&Peer>, m: &Peer| {
        for _ in 0..8 {
            b.node.flush_escrow_outbox(true);
            if let Some(s) = s {
                s.node.flush_escrow_outbox(true);
            }
            m.node.flush_escrow_outbox(true);
            let mut n = b.node.process_escrow_mail() + m.node.process_escrow_mail();
            if let Some(s) = s {
                n += s.node.process_escrow_mail();
            }
            if n == 0 {
                break;
            }
        }
    };

    pump(&buyer, Some(&seller), &moderator);
    fund_node_wallet(&buyer.node, &chain, 200_000);
    let (status, paid) = pay(&buyer.node, &order_id, "{}");
    assert!(status.contains("202"), "{status} {paid}");
    pump(&buyer, Some(&seller), &moderator);
    chain.mine();
    buyer.node.watch_once().unwrap();
    assert_eq!(order(&buyer.node, &order_id)["status"], FUNDED);

    // Export locally retained contract via GET /orders/{id}/contract.
    let (status, exported_json) = request(&buyer.node, "GET", &format!("/orders/{order_id}/contract"), "");
    assert!(status.contains("200"), "{status} {exported_json}");
    let contract = RetainedContract::from_json(&exported_json).unwrap();
    assert_eq!(contract.network, "testnet4");
    assert!(contract.escrow.as_ref().unwrap().buyer_snapshot_hex.is_some());

    // Recover buyer's 12-word phrase and derive BuyerSeed from scratch.
    let (_, phrase_val) = request(
        &buyer.node,
        "POST",
        "/wallet/phrase",
        &json!({ "passphrase": crate::harness::PASSPHRASE }).to_string(),
    );
    let phrase = phrase_val["phrase"].as_str().unwrap();
    let recovered = BuyerSeed::from_phrase(phrase, NetworkMode::Testnet4).unwrap();

    // Tampered escrow address MUST fail verification!
    let mut tampered = contract.clone();
    tampered.escrow.as_mut().unwrap().funding_address = recovered.payout_address(9).to_string();
    let err = tampered.verify_escrow(&recovered).err().unwrap();
    assert!(err.contains("does not match the signed MuSig2 terms"), "{err}");
    assert!(tampered.build_offline_dispute(&recovered, "Seller disappeared.", &buyer.url).is_err());

    // Now the scam seller disappears and shuts down their node completely!
    seller.stop();
    drop(seller);

    // Buyer constructs the signed dispute payload locally from the 12-word seed + retained contract
    // and posts it straight to the moderator without touching the dead seller.
    let dispute = contract
        .build_offline_dispute(&recovered, "Scam seller vanished after payment.", &buyer.url)
        .unwrap();
    let (mod_status, mod_body) = post_raw(&moderator.node, "/escrow/mail", &dispute.sealed_mail_bytes, None);
    assert!(mod_status.contains("202"), "{mod_status} {mod_body}");

    // Moderator opens and verifies the buyer's offline dispute against the chain!
    assert_eq!(moderator.node.process_escrow_mail(), 1);
    let (status, disputes) = request(&moderator.node, "GET", "/moderator/disputes", "");
    assert!(status.contains("200"), "{status} {disputes}");
    let list = disputes["disputes"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{disputes}");
    assert_eq!(list[0]["id"], dispute.escrow_id);
    assert_eq!(list[0]["memo"], "Scam seller vanished after payment.");
}

#[test]
fn mainnet_escrow_refusal_and_testnet4_default_remain_intact() {
    assert_eq!(ServeOptions::parse(&[]).unwrap().network, NetworkMode::Testnet4);
    let dir = TempDir::new();
    let main = open_node(&dir.0, NetworkMode::Mainnet, &MockChain::new(), 1, EscrowConfig::default());
    assert_eq!(
        main.escrow_ready("Any Seller").err().map(|e| e.to_string()).as_deref(),
        Some(ESCROW_MAINNET_REFUSAL)
    );
    let main_seed = BuyerSeed::from_phrase(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        NetworkMode::Mainnet,
    )
    .unwrap();
    let contract = RetainedContract {
        version: 1,
        network: "mainnet".into(),
        order_id: "o".into(),
        checkout_id: "c".into(),
        store_id: "00".repeat(32),
        seller_url: String::new(),
        store_record_hex: String::new(),
        listing_records_hex: vec![],
        escrow: None,
    };
    assert_eq!(contract.verify_escrow(&main_seed).err().as_deref(), Some(ESCROW_MAINNET_REFUSAL));
    assert_eq!(
        contract.build_offline_dispute(&main_seed, "missing", "http://127.0.0.1:9").err().as_deref(),
        Some(ESCROW_MAINNET_REFUSAL)
    );
}

#[test]
fn cross_platform_test_vectors_match_mobile_buyer_light() {
    // Deterministic vectors pinned between Rust `buyer_light` and TypeScript `apps/mobile/src/buyerLight/`.
    const PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let seed = BuyerSeed::from_phrase(PHRASE, NetworkMode::Testnet4).unwrap();
    let store_id = [0x42u8; 32];
    let keys = seed.order_keys("checkout-vector-1", &store_id);
    let (_, escrow_pub0) = seed.escrow_key(0);
    let (_, escrow_pub1) = seed.escrow_key(1);
    let payout0 = seed.payout_address(0).to_string();

    // Verify MuSig2 Taproot address derivation for (escrow_pub0, escrow_pub1) on testnet4.
    let p0 = musig2::secp::Point::from_slice(&escrow_pub0).unwrap();
    let p1 = musig2::secp::Point::from_slice(&escrow_pub1).unwrap();
    let mut sorted = [p0, p1];
    sorted.sort_by_key(|p| p.serialize());
    let ctx = musig2::KeyAggContext::new(sorted)
        .unwrap()
        .with_unspendable_taproot_tweak()
        .unwrap();
    let out_xonly = ctx.aggregated_pubkey::<musig2::secp::Point>().serialize_xonly();
    let escrow_addr = bitcoin::Address::p2tr_tweaked(
        bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(
            bitcoin::secp256k1::XOnlyPublicKey::from_slice(&out_xonly).unwrap(),
        ),
        bitcoin::Network::Testnet4,
    )
    .to_string();

    // Pull request & sealed reply round-trip with deterministic seed keys.
    let store_sec = tormail::StoreSecret::from_bytes([0x11u8; 32]).unwrap();
    let nonce = [0x22u8; 16];
    let pull = build_pull_request(&keys.drop_id.0, &keys.sender, &store_sec.public_key(), &nonce).unwrap();
    let opened_req = tormail::open_message(&pull, &store_sec).unwrap();
    assert_eq!(opened_req.sender, keys.sender.id());
    let seller_key = SenderKey::generate();
    let reply_body = json!({ "v": 1, "kind": "status", "nonce": hex(&nonce), "status": "Paid" });
    let sealed_reply = tormail::seal_reply(&serde_json::to_vec(&reply_body).unwrap(), &keys.reply_public, &seller_key).unwrap();
    let (seller_hex, got) = open_pull_response(&sealed_reply.to_bytes(), &keys.reply_secret, &nonce, None).unwrap();
    assert_eq!(seller_hex, hex(&seller_key.id().0));
    assert_eq!(got["status"], "Paid");

    // Pin exact deterministic values verified identically in `apps/mobile/src/buyerLight/buyerLight.test.ts`.
    assert_eq!(hex(&seed.letter_root()), "ca73e4a217e2218c33f5ee108aeaa28289eafe45a220c95460767968e2d681b7");
    assert_eq!(hex(&keys.sender.id().0), "843bc15a4e91f8e97b12aa994b5d9b2509525e9b2d935c47cf1a707369d04d4b");
    assert_eq!(hex(keys.reply_public.as_bytes()), "4909ab310de31e2e7507489fb66cfa96137576a64b40dd212e0e7a203f28d600");
    assert_eq!(hex(&keys.drop_id.0), "f45bf1474cf32ec30a7ed187e2aafa44");
    assert_eq!(hex(&escrow_pub0), "02e4cdef6b21c4e92cf3c51e9f2aff9d5c887ea96422601ff10f11082f60dfafc7");
    assert_eq!(hex(&escrow_pub1), "02301c01774b5dd8fd8e0af46781485e86bb59656e2d026b3ec71a9051940bdbed");
    assert_eq!(payout0, "tb1p8wpt9v4frpf3tkn0srd97pksgsxc5hs52lafxwru9kgeephvs7rqlqt9zj");
    assert_eq!(escrow_addr, "tb1pjehacjjpsthxk3u0yvpan39zmhl8zwwcxaujz0h7ta9karnjgatq8tnkd0");
}
