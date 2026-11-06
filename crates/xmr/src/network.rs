//! The two Monero networks the node uses.
//!
//! There is no Monero testnet4. The node's testnet4 mode pays on stagenet,
//! which follows mainnet's consensus rules with coins of no value. The
//! mapping lives in the node (`NetworkMode::monero`), so it is written down
//! once and tested there.

use monero_wallet::address::Network;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XmrNetwork {
    Mainnet,
    Stagenet,
}

/// Block 0 of each network. A daemon whose genesis differs is on another network.
const MAINNET_GENESIS: &str = "418015bb9ae982a1975da7d79277c2705727a56894ba0fb246adaabb1f4632e3";
const STAGENET_GENESIS: &str = "76ee3cc98646292206cd3e86f74d88b4dcc1d937088645e9b0cbca84b7ce74eb";

impl XmrNetwork {
    pub fn as_str(self) -> &'static str {
        match self {
            XmrNetwork::Mainnet => "mainnet",
            XmrNetwork::Stagenet => "stagenet",
        }
    }

    pub fn address_network(self) -> Network {
        match self {
            XmrNetwork::Mainnet => Network::Mainnet,
            XmrNetwork::Stagenet => Network::Stagenet,
        }
    }

    /// monerod's default RPC port.
    pub fn default_rpc_port(self) -> u16 {
        match self {
            XmrNetwork::Mainnet => 18081,
            XmrNetwork::Stagenet => 38081,
        }
    }

    pub fn genesis_hash(self) -> [u8; 32] {
        let hex = match self {
            XmrNetwork::Mainnet => MAINNET_GENESIS,
            XmrNetwork::Stagenet => STAGENET_GENESIS,
        };
        let mut out = [0u8; 32];
        hex::decode_to_slice(hex, &mut out).expect("genesis hashes are 32-byte hex");
        out
    }
}

impl std::fmt::Display for XmrNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
