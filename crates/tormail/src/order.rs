use std::collections::BTreeMap;
use std::fmt;

use crate::onion::OnionAddress;
use crate::reply::{DropId, ReplyPath};
use crate::seal::ReplyPublic;
use crate::Error;

const MAGIC: &[u8; 4] = b"OBOD";
/// Version 1 carried only a reply key. Version 2 carries a [`ReplyPath`].
const VERSION_REPLY: u8 = 2;
/// Version 3 adds [`OrderTerms`]. An order with default terms is still
/// written as version 2, so a seller on older code can read it.
const VERSION: u8 = 3;
const MAX_TERMS_BLOB: usize = 512;
/// [`OrderTerms::service`] when nothing is shipped: pickup or digital.
pub const NO_SERVICE: u8 = 0xff;
const MAX_QUANTITY: u32 = 1_000_000;
const MAX_OPTIONS: usize = 16;
const MAX_OPTION_NAME: usize = 64;
const MAX_OPTION_VALUE: usize = 256;
const MAX_SHIP_TO: usize = 1024;
const MAX_ONION: usize = 62;

/// Hash of the public listing the buyer is ordering.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListingHash(pub [u8; 32]);

impl ListingHash {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ListingHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ListingHash")
            .field(&hex(&self.0))
            .finish()
    }
}

/// Direct-pay rail the buyer wants the vendor to use.
///
/// Crypto here chooses how the buyer will pay. It does not conceal a
/// package. The ship-to address is a separate field the vendor reads
/// when they open the letter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaymentRail {
    /// BOLT 12 Lightning.
    Lightning = 1,
    /// BIP 352 on-chain silent payment.
    OnChain = 2,
}

impl PaymentRail {
    fn from_byte(byte: u8) -> Result<Self, Error> {
        match byte {
            1 => Ok(Self::Lightning),
            2 => Ok(Self::OnChain),
            _ => Err(Error::InvalidOrder("unknown payment rail")),
        }
    }

    fn as_byte(self) -> u8 {
        self as u8
    }
}

/// A private order, sealed to one vendor.
///
/// `ship_to` is ordinary address text. The vendor is supposed to learn it
/// by opening the letter. Nothing in this struct is a dead-drop or a
/// hidden-delivery instruction.
#[derive(Clone, PartialEq, Eq)]
pub struct Order {
    pub listing_hash: ListingHash,
    pub quantity: u32,
    pub options: BTreeMap<String, String>,
    pub ship_to: String,
    pub rail: PaymentRail,
    /// Per-order reply key, reply onion, and drop id. See [`crate::reply`].
    pub reply: ReplyPath,
    pub terms: OrderTerms,
}

/// What a version 3 order adds: which variant, how it ships, and any coupon
/// or accepted offer, as encoded signed records the vendor checks itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrderTerms {
    /// Stable listing id; zeros for a listing without one.
    pub listing_id: [u8; 16],
    /// Zeros for a listing without variants.
    pub variant_id: [u8; 8],
    /// Index into the listing's shipping services, or [`NO_SERVICE`].
    pub service: u8,
    /// An encoded signed coupon, or empty.
    pub coupon: Vec<u8>,
    /// An encoded signed offer, or empty.
    pub offer: Vec<u8>,
    /// The order's review key: a receipt names it, a review is signed by it.
    pub review_key: [u8; 32],
}

impl OrderTerms {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Order {
    /// Build an order, rejecting empty or oversized fields.
    pub fn new(
        listing_hash: ListingHash,
        quantity: u32,
        options: BTreeMap<String, String>,
        ship_to: impl Into<String>,
        rail: PaymentRail,
        reply: ReplyPath,
    ) -> Result<Self, Error> {
        let order = Self {
            listing_hash,
            quantity,
            options,
            ship_to: ship_to.into(),
            rail,
            reply,
            terms: OrderTerms::default(),
        };
        order.check()?;
        Ok(order)
    }

    /// The same order with version 3 terms.
    pub fn with_terms(self, terms: OrderTerms) -> Result<Self, Error> {
        let order = Self { terms, ..self };
        order.check()?;
        Ok(order)
    }

    /// Re-check fields. `seal_order` calls this because the struct fields are public.
    pub(crate) fn check(&self) -> Result<(), Error> {
        if self.quantity == 0 || self.quantity > MAX_QUANTITY {
            return Err(Error::InvalidOrder("quantity out of range"));
        }
        if self.options.len() > MAX_OPTIONS {
            return Err(Error::InvalidOrder("too many options"));
        }
        for (name, value) in &self.options {
            check_text(name, 1, MAX_OPTION_NAME, "option name")?;
            check_text(value, 0, MAX_OPTION_VALUE, "option value")?;
        }
        check_text(&self.ship_to, 1, MAX_SHIP_TO, "ship-to")?;
        if self.ship_to.trim().is_empty() {
            return Err(Error::InvalidOrder("ship-to is empty"));
        }
        if self.terms.coupon.len() > MAX_TERMS_BLOB || self.terms.offer.len() > MAX_TERMS_BLOB {
            return Err(Error::InvalidOrder("coupon or offer is too long"));
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.ship_to.len());
        out.extend_from_slice(MAGIC);
        out.push(if self.terms.is_default() { VERSION_REPLY } else { VERSION });
        out.extend_from_slice(&self.listing_hash.0);
        out.extend_from_slice(&self.quantity.to_le_bytes());
        out.extend_from_slice(&(self.options.len() as u16).to_le_bytes());
        for (name, value) in &self.options {
            push_str(&mut out, name);
            push_str(&mut out, value);
        }
        push_str(&mut out, &self.ship_to);
        out.push(self.rail.as_byte());
        out.extend_from_slice(self.reply.key.as_bytes());
        push_str(&mut out, self.reply.onion.as_ref().map_or("", OnionAddress::as_str));
        out.extend_from_slice(&self.reply.drop.0);
        if !self.terms.is_default() {
            let terms = &self.terms;
            out.extend_from_slice(&terms.listing_id);
            out.extend_from_slice(&terms.variant_id);
            out.push(terms.service);
            push_blob(&mut out, &terms.coupon);
            push_blob(&mut out, &terms.offer);
            out.extend_from_slice(&terms.review_key);
        }
        out
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut cursor = Cursor::new(bytes);
        let magic = cursor.take(4)?;
        if magic != MAGIC {
            return Err(Error::Malformed);
        }
        let version = cursor.u8()?;
        if version != VERSION_REPLY && version != VERSION {
            return Err(Error::Malformed);
        }
        let listing_hash = ListingHash(cursor.array()?);
        let quantity = cursor.u32()?;
        let option_count = cursor.u16()? as usize;
        if option_count > MAX_OPTIONS {
            return Err(Error::InvalidOrder("too many options"));
        }
        let mut options = BTreeMap::new();
        for _ in 0..option_count {
            let name = cursor.string(MAX_OPTION_NAME)?;
            let value = cursor.string(MAX_OPTION_VALUE)?;
            if name.is_empty() {
                return Err(Error::InvalidOrder("option name is empty"));
            }
            if options.insert(name, value).is_some() {
                return Err(Error::InvalidOrder("duplicate option name"));
            }
        }
        let ship_to = cursor.string(MAX_SHIP_TO)?;
        let rail = PaymentRail::from_byte(cursor.u8()?)?;
        let key = ReplyPublic::from_bytes(cursor.array()?)?;
        let onion = cursor.string(MAX_ONION)?;
        let onion = if onion.is_empty() { None } else { Some(OnionAddress::parse(&onion)?) };
        let drop = DropId(cursor.array()?);
        let mut terms = OrderTerms::default();
        if version == VERSION {
            terms.listing_id = cursor.array()?;
            terms.variant_id = cursor.array()?;
            terms.service = cursor.u8()?;
            terms.coupon = cursor.blob(MAX_TERMS_BLOB)?;
            terms.offer = cursor.blob(MAX_TERMS_BLOB)?;
            terms.review_key = cursor.array()?;
            if terms.is_default() {
                return Err(Error::Malformed);
            }
        }
        if !cursor.is_empty() {
            return Err(Error::Malformed);
        }
        Self::new(listing_hash, quantity, options, ship_to, rail, ReplyPath { key, onion, drop })?.with_terms(terms)
    }
}

impl fmt::Debug for Order {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Order")
            .field("listing_hash", &self.listing_hash)
            .field("quantity", &self.quantity)
            .field("options", &self.options)
            .field("ship_to", &"(sealed from logs)")
            .field("rail", &self.rail)
            .field("reply", &self.reply)
            .field("terms", &self.terms)
            .finish()
    }
}

fn check_text(value: &str, min: usize, max: usize, what: &'static str) -> Result<(), Error> {
    if value.len() < min || value.len() > max || value.as_bytes().contains(&0) {
        return Err(Error::InvalidOrder(match what {
            "option name" => "option name is empty or too long",
            "option value" => "option value is too long",
            "ship-to" => "ship-to is empty or too long",
            _ => "invalid text field",
        }));
    }
    Ok(())
}

fn push_blob(out: &mut Vec<u8>, value: &[u8]) {
    let len = u16::try_from(value.len()).expect("blob limit fits in u16");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value);
}

fn push_str(out: &mut Vec<u8>, value: &str) {
    let len = u16::try_from(value.len()).expect("field limit fits in u16");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(len).ok_or(Error::Malformed)?;
        if end > self.bytes.len() {
            return Err(Error::Malformed);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let bytes = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn blob(&mut self, max: usize) -> Result<Vec<u8>, Error> {
        let len = self.u16()? as usize;
        if len > max {
            return Err(Error::InvalidOrder("coupon or offer is too long"));
        }
        Ok(self.take(len)?.to_vec())
    }

    fn string(&mut self, max: usize) -> Result<String, Error> {
        let len = self.u16()? as usize;
        if len > max {
            return Err(Error::InvalidOrder("text field is too long"));
        }
        let bytes = self.take(len)?;
        if bytes.contains(&0) {
            return Err(Error::InvalidOrder("text field contains NUL"));
        }
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::InvalidOrder("text field is not utf-8"))
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::ReplyKeypair;

    fn order() -> Order {
        let mut options = BTreeMap::new();
        options.insert("Size".to_owned(), "M".to_owned());
        let reply = ReplyKeypair::generate();
        Order::new(
            ListingHash([7; 32]),
            2,
            options,
            "1 Test Lane",
            PaymentRail::OnChain,
            ReplyPath::sealed_only(*reply.public_key(), DropId::generate()),
        )
        .unwrap()
    }

    fn terms() -> OrderTerms {
        OrderTerms {
            listing_id: [9; 16],
            variant_id: [4; 8],
            service: 1,
            coupon: b"OBCP-bytes".to_vec(),
            offer: Vec::new(),
            review_key: [3; 32],
        }
    }

    #[test]
    fn default_terms_stay_version_2() {
        let order = order();
        let bytes = order.encode();
        assert_eq!(bytes[4], VERSION_REPLY);
        assert_eq!(Order::decode(&bytes).unwrap(), order);
    }

    #[test]
    fn terms_round_trip_as_version_3() {
        let order = order().with_terms(terms()).unwrap();
        let bytes = order.encode();
        assert_eq!(bytes[4], VERSION);
        let decoded = Order::decode(&bytes).unwrap();
        assert_eq!(decoded.terms, terms());
        assert_eq!(decoded, order);
    }

    #[test]
    fn version_3_with_default_terms_is_not_canonical() {
        let mut bytes = order().encode();
        bytes[4] = VERSION;
        bytes.extend_from_slice(&[0; 16 + 8 + 1 + 2 + 2 + 32]);
        assert!(matches!(Order::decode(&bytes), Err(Error::Malformed)));
    }

    #[test]
    fn oversized_blobs_are_refused() {
        let big = OrderTerms { offer: vec![1; MAX_TERMS_BLOB + 1], ..terms() };
        assert!(matches!(order().with_terms(big), Err(Error::InvalidOrder(_))));
    }
}
