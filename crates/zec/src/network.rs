//! The two Zcash networks the node uses.
//!
//! The node's testnet4 mode pays on Zcash testnet. The mapping lives in the
//! node (as `NetworkMode::monero` does for Monero), so it is written down once.

use zcash_protocol::consensus::Network;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZecNetwork {
    Mainnet,
    Testnet,
}

impl ZecNetwork {
    pub fn as_str(self) -> &'static str {
        match self {
            ZecNetwork::Mainnet => "mainnet",
            ZecNetwork::Testnet => "testnet",
        }
    }

    /// Consensus parameters: activation heights, address prefixes, the ZIP 32 coin type.
    pub fn params(self) -> Network {
        match self {
            ZecNetwork::Mainnet => Network::MainNetwork,
            ZecNetwork::Testnet => Network::TestNetwork,
        }
    }

    /// The `chainName` lightwalletd and Zaino report from `GetLightdInfo`.
    /// A server that reports another is on another network.
    pub fn chain_name(self) -> &'static str {
        match self {
            ZecNetwork::Mainnet => "main",
            ZecNetwork::Testnet => "test",
        }
    }
}

impl std::fmt::Display for ZecNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
