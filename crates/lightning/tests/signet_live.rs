//! A live check against Mutinynet (a public signet) and its public LSPS2 LSP.
//! Ignored: it needs the network. Run by hand:
//!
//! ```text
//! cargo test -p openbazaar-lightning --test signet_live -- --ignored --nocapture
//! ```
//!
//! It starts a fresh LDK node, asks the LSP for a just-in-time invoice, asks
//! the Mutinynet faucet to pay it, and waits for the payment. Set
//! `OB_LN_LIVE_DIR` to keep the node's state between runs.

use std::time::{Duration, Instant};

use openbazaar_lightning::{ChainSource, GossipSource, Incoming, LdkRail, LightningRail, LnConfig, LspConfig};

const LSP: &str = "0371d6fd7d75de2d0372d03ea00e8bacdacb50c27d0eaea0a76a0622eff1f5ef2b@44.219.111.31:39735";
const LSP_TOKEN: &str = "JZWN9YLW";

#[test]
#[ignore]
fn mutinynet_lsp_opens_a_channel_for_a_faucet_payment() {
    let temp = tempfile::tempdir().unwrap();
    let dir = std::env::var_os("OB_LN_LIVE_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| temp.path().to_path_buf());
    let config = LnConfig {
        network: ldk_node::bitcoin::Network::Signet,
        storage_dir: dir,
        chain: ChainSource::Esplora("https://mutinynet.com/api".into()),
        gossip: GossipSource::Rgs("https://rgs.mutinynet.com/snapshot".into()),
        lsp: Some(LspConfig { peer: LSP.parse().unwrap(), token: Some(LSP_TOKEN.into()) }),
        listen: None,
        max_lsp_fee_bps: 500,
        min_lsp_fee_cap_sats: 15_000,
    };
    let seller = LdkRail::start(config, &[0x5a; 32]).unwrap();
    println!("node id {}", seller.info().node_id);
    let invoice = seller.receive(50_000_000, "OpenBazaar live check", 3_600).unwrap();
    println!("invoice (jit {}) {}", invoice.jit, invoice.bolt11);
    println!("payment hash {}", invoice.payment_hash);

    let faucet = std::process::Command::new("curl")
        .args(["-s", "-m", "60", "-X", "POST", "-H", "Content-Type: application/json"])
        .arg("-d")
        .arg(format!("{{\"bolt11\":\"{}\"}}", invoice.bolt11))
        .arg("https://faucet.mutinynet.com/api/lightning")
        .output()
        .unwrap();
    println!("faucet: {}", String::from_utf8_lossy(&faucet.stdout));

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(300) {
        if let Incoming::Paid { received_msat, lsp_fee_msat, .. } = seller.incoming(&invoice.payment_hash) {
            println!("paid: received {received_msat} msat, LSP kept {lsp_fee_msat} msat");
            println!("channels {:?}", seller.channels());
            seller.stop();
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    seller.stop();
    panic!("not paid within 5 minutes; pay the invoice above from any Mutinynet wallet to finish the check");
}
