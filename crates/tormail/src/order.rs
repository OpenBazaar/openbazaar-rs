use std::collections::BTreeMap;
use std::fmt;

use crate::seal::ReplyPublic;
use crate::Error;

const MAGIC: &[u8; 4] = b"OBOD";
const VERSION: u8 = 1;
const MAX_QUANTITY: u32 = 1_000_000;
const MAX_OPTIONS: usize = 16;
const MAX_OPTION_NAME: usize = 64;
const MAX_OPTION_VALUE: usize = 256;
const MAX_SHIP_TO: usize = 1024;

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
    /// Ephemeral X25519 key the vendor uses to answer this buyer directly.
    pub reply_key: ReplyPublic,
}

impl Order {
    /// Build an order, rejecting empty or oversized fields.
    pub fn new(
        listing_hash: ListingHash,
        quantity: u32,
        options: BTreeMap<String, String>,
        ship_to: impl Into<String>,
        rail: PaymentRail,
        reply_key: ReplyPublic,
    ) -> Result<Self, Error> {
        let order = Self {
            listing_hash,
            quantity,
            options,
            ship_to: ship_to.into(),
            rail,
            reply_key,
        };
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
        Ok(())
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.ship_to.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.listing_hash.0);
        out.extend_from_slice(&self.quantity.to_le_bytes());
        out.extend_from_slice(&(self.options.len() as u16).to_le_bytes());
        for (name, value) in &self.options {
            push_str(&mut out, name);
            push_str(&mut out, value);
        }
        push_str(&mut out, &self.ship_to);
        out.push(self.rail.as_byte());
        out.extend_from_slice(self.reply_key.as_bytes());
        out
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut cursor = Cursor::new(bytes);
        let magic = cursor.take(4)?;
        if magic != MAGIC {
            return Err(Error::Malformed);
        }
        if cursor.u8()? != VERSION {
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
        let reply_key = ReplyPublic::from_bytes(cursor.array()?)?;
        if !cursor.is_empty() {
            return Err(Error::Malformed);
        }
        Self::new(listing_hash, quantity, options, ship_to, rail, reply_key)
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
            .field("reply_key", &self.reply_key)
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
