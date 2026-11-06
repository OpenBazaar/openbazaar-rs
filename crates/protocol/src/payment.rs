/// Direct pay shows both rails at once. There is no swap in the middle.
///
/// The strings identify a BOLT 12 offer and a BIP 352 silent-payment code.
/// This phase does not parse them, fetch an invoice, or construct a payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectPay {
    pub bolt12_offer: String,
    pub silent_payment_code: String,
}
