//! ZIP 321 `zcash:` URIs for external wallets.

use zcash_address::ZcashAddress;
use zcash_protocol::value::Zatoshis;
use zip321::{Payment, TransactionRequest};

/// `zcash:<address>?amount=<ZEC>`. No label, message, or memo: whatever is
/// in the URI ends up in the payer's wallet history.
pub fn zcash_uri(address: &str, zatoshi: u64) -> Result<String, String> {
    let address = ZcashAddress::try_from_encoded(address).map_err(|_| "That is not a Zcash address.".to_string())?;
    let amount = Zatoshis::from_u64(zatoshi).map_err(|_| "That amount is more ZEC than exists.".to_string())?;
    let request = TransactionRequest::new(vec![Payment::without_memo(address, amount)])
        .map_err(|err| format!("That payment request is not valid: {err}"))?;
    Ok(request.to_uri())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ViewKey, ZecNetwork};

    #[test]
    fn uri_carries_the_address_and_amount_only() {
        let address = ViewKey::from_seed(&[7u8; 64], ZecNetwork::Mainnet).unwrap().address(3);
        let uri = zcash_uri(&address, 1_463_415).unwrap();
        assert_eq!(uri, format!("zcash:{address}?amount=0.01463415"));
        let parsed = TransactionRequest::from_uri(&uri).unwrap();
        let payment = &parsed.payments()[&0];
        assert_eq!(payment.amount(), Some(Zatoshis::from_u64(1_463_415).unwrap()));
        assert!(payment.memo().is_none() && payment.label().is_none() && payment.message().is_none());
        assert!(zcash_uri("t1nope", 1).is_err());
        assert!(zcash_uri(&address, u64::MAX).is_err());
    }
}
