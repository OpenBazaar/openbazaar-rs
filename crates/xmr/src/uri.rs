//! `monero:` URIs for external wallets.

use monero_wallet::address::MoneroAddress;

use crate::amount::format_xmr;

/// `monero:<address>?tx_amount=<XMR>`. No description or recipient name:
/// whatever is in the URI ends up in the payer's wallet history.
pub fn monero_uri(address: &MoneroAddress, piconero: u64) -> String {
    format!("monero:{address}?tx_amount={}", format_xmr(piconero))
}
