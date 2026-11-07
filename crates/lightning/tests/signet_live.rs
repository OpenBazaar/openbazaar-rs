//! The public Mutinynet LSP is not dialed when no proxy is set. This ignored
//! test must keep failing closed: a public peer is reached only through
//! `--proxy`, and this config does not set one.

use openbazaar_lightning::{ChainSource, GossipSource, LdkRail, LightningRail, LnConfig, LspConfig};

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
        proxy: None,
    };
    match LdkRail::start(config, &[0x5a; 32]) {
        Err(err) => assert!(err.to_string().contains("in the clear"), "{err}"),
        Ok(rail) => {
            rail.stop();
            panic!("a public Lightning peer was started");
        }
    }
}
