use std::fmt;

use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{
    setup_receiver, setup_sender, Deserializable, Kem as KemTrait, OpModeR, OpModeS, Serializable,
};
use rand::rngs::StdRng;
use rand::SeedableRng;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::order::Order;
use crate::Error;

type Aead = ChaCha20Poly1305;
type Kdf = HkdfSha256;
type Suite = X25519HkdfSha256;
type Public = <Suite as KemTrait>::PublicKey;
type Secret = <Suite as KemTrait>::PrivateKey;
type Encapped = <Suite as KemTrait>::EncappedKey;

const ORDER_INFO: &[u8] = b"openbazaar-tormail-order-v1";
const REPLY_INFO: &[u8] = b"openbazaar-tormail-reply-v1";
const MESSAGE_INFO: &[u8] = b"openbazaar-tormail-message-v1";
const LETTER_MAGIC: &[u8; 4] = b"OBSL";
const REPLY_MAGIC: &[u8; 4] = b"OBSR";
const VERSION: u8 = 1;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;
/// Largest sealed blob the mailbox and the onion frame will accept.
pub const MAX_SEALED_LEN: usize = 64 * 1024;

/// Vendor store public key. Orders are sealed to this key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreKey([u8; KEY_LEN]);

/// Vendor store secret. Only this key opens an order letter.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct StoreSecret([u8; KEY_LEN]);

/// A vendor store keypair. The public half is published with the shop.
/// The secret stays on the vendor's machines and is not required to run
/// the onion listener.
#[derive(Clone)]
pub struct StoreKeypair {
    public: StoreKey,
    secret: StoreSecret,
}

/// Ephemeral X25519 public key carried inside the sealed order.
///
/// The vendor uses it to answer the buyer directly. There is no relay
/// that has to remember who wrote to whom.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReplyPublic([u8; KEY_LEN]);

/// Secret half of [`ReplyPublic`]. The buyer keeps this; it is not sealed
/// into the order.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ReplySecret([u8; KEY_LEN]);

/// Buyer reply keypair generated for one order.
#[derive(Clone)]
pub struct ReplyKeypair {
    public: ReplyPublic,
    secret: ReplySecret,
}

/// An order encrypted to one vendor store key.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedLetter {
    enc: [u8; KEY_LEN],
    ciphertext: Vec<u8>,
}

/// A short reply encrypted to the buyer's ephemeral reply key.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedReply {
    enc: [u8; KEY_LEN],
    ciphertext: Vec<u8>,
}

impl StoreKey {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Result<Self, Error> {
        parse_public(&bytes)?;
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl StoreSecret {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Result<Self, Error> {
        parse_secret(&bytes)?;
        Ok(Self(bytes))
    }

    pub fn to_bytes(&self) -> [u8; KEY_LEN] {
        self.0
    }

    pub fn public_key(&self) -> StoreKey {
        StoreKey(public_of(&self.0))
    }
}

impl StoreKeypair {
    pub fn generate() -> Self {
        let (secret, public) = generate();
        Self {
            public: StoreKey(public),
            secret: StoreSecret(secret),
        }
    }

    pub fn public(&self) -> &StoreKey {
        &self.public
    }

    pub fn secret(&self) -> &StoreSecret {
        &self.secret
    }
}

impl ReplyPublic {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Result<Self, Error> {
        parse_public(&bytes)?;
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl ReplySecret {
    pub fn public_key(&self) -> ReplyPublic {
        ReplyPublic(public_of(&self.0))
    }
}

impl ReplyKeypair {
    pub fn generate() -> Self {
        let (secret, public) = generate();
        Self {
            public: ReplyPublic(public),
            secret: ReplySecret(secret),
        }
    }

    pub fn public_key(&self) -> &ReplyPublic {
        &self.public
    }

    pub fn secret(&self) -> &ReplySecret {
        &self.secret
    }
}

impl SealedLetter {
    pub fn from_parts(enc: [u8; KEY_LEN], ciphertext: Vec<u8>) -> Result<Self, Error> {
        check_ciphertext(&ciphertext)?;
        Ok(Self { enc, ciphertext })
    }

    pub fn encap_key(&self) -> &[u8; KEY_LEN] {
        &self.enc
    }

    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        encode_blob(LETTER_MAGIC, &self.enc, &self.ciphertext)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let (enc, ciphertext) = decode_blob(LETTER_MAGIC, bytes)?;
        Self::from_parts(enc, ciphertext)
    }
}

impl SealedReply {
    pub fn to_bytes(&self) -> Vec<u8> {
        encode_blob(REPLY_MAGIC, &self.enc, &self.ciphertext)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let (enc, ciphertext) = decode_blob(REPLY_MAGIC, bytes)?;
        check_ciphertext(&ciphertext)?;
        Ok(Self { enc, ciphertext })
    }
}

/// Seal `order` so only `vendor`'s secret can open it.
///
/// The buyer's reply public key is inside the plaintext. The HPKE
/// ephemeral used for this seal is separate and is not a buyer identity.
pub fn seal_order(order: &Order, vendor: &StoreKey) -> Result<SealedLetter, Error> {
    order.check()?;
    let (enc, ciphertext) = seal_to(&vendor.0, ORDER_INFO, &order.encode())?;
    SealedLetter::from_parts(enc, ciphertext)
}

/// Open a letter with the vendor store secret.
pub fn open_order(letter: &SealedLetter, secret: &StoreSecret) -> Result<Order, Error> {
    let plaintext = open_with(&secret.0, ORDER_INFO, &letter.enc, &letter.ciphertext)?;
    Order::decode(&plaintext)
}

/// Largest body [`seal_message`] accepts: the sealed blob stays within [`MAX_SEALED_LEN`].
pub const MAX_MESSAGE_LEN: usize = MAX_SEALED_LEN - (4 + 1 + KEY_LEN + 4) - TAG_LEN;

/// Seal an opaque message (not an order) so only `to`'s secret can open it.
///
/// It is a [`SealedLetter`] like an order, under a separate HPKE context, so
/// [`open_order`] refuses a message and [`open_message`] refuses an order.
pub fn seal_message(body: &[u8], to: &StoreKey) -> Result<SealedLetter, Error> {
    if body.is_empty() || body.len() > MAX_MESSAGE_LEN {
        return Err(Error::InvalidOrder("message body is empty or too long"));
    }
    let (enc, ciphertext) = seal_to(&to.0, MESSAGE_INFO, body)?;
    SealedLetter::from_parts(enc, ciphertext)
}

/// Open a [`seal_message`] letter.
pub fn open_message(letter: &SealedLetter, secret: &StoreSecret) -> Result<Vec<u8>, Error> {
    open_with(&secret.0, MESSAGE_INFO, &letter.enc, &letter.ciphertext)
}

/// Seal a vendor reply to the ephemeral key found in an opened order.
pub fn seal_reply(body: &[u8], buyer: &ReplyPublic) -> Result<SealedReply, Error> {
    if body.is_empty() || body.len() > 8 * 1024 {
        return Err(Error::InvalidOrder("reply body is empty or too long"));
    }
    let (enc, ciphertext) = seal_to(&buyer.0, REPLY_INFO, body)?;
    Ok(SealedReply { enc, ciphertext })
}

/// Open a vendor reply with the buyer's ephemeral secret.
pub fn open_reply(reply: &SealedReply, secret: &ReplySecret) -> Result<Vec<u8>, Error> {
    open_with(&secret.0, REPLY_INFO, &reply.enc, &reply.ciphertext)
}

fn generate() -> ([u8; KEY_LEN], [u8; KEY_LEN]) {
    let mut rng = StdRng::from_os_rng();
    let (secret, public) = Suite::gen_keypair(&mut rng);
    (to_key(&secret), to_key(&public))
}

fn public_of(secret: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    let secret = Secret::from_bytes(secret).expect("secret length was checked");
    to_key(&Suite::sk_to_pk(&secret))
}

fn parse_public(bytes: &[u8; KEY_LEN]) -> Result<Public, Error> {
    Public::from_bytes(bytes).map_err(|_| Error::Malformed)
}

fn parse_secret(bytes: &[u8; KEY_LEN]) -> Result<Secret, Error> {
    Secret::from_bytes(bytes).map_err(|_| Error::Malformed)
}

fn seal_to(
    recipient: &[u8; KEY_LEN],
    info: &[u8],
    plaintext: &[u8],
) -> Result<([u8; KEY_LEN], Vec<u8>), Error> {
    let public = parse_public(recipient)?;
    let mut rng = StdRng::from_os_rng();
    let (enc, mut ctx) =
        setup_sender::<Aead, Kdf, Suite, _>(&OpModeS::Base, &public, info, &mut rng)
            .map_err(|_| Error::Seal)?;
    let ciphertext = ctx.seal(plaintext, b"").map_err(|_| Error::Seal)?;
    Ok((to_key(&enc), ciphertext))
}

fn open_with(
    recipient: &[u8; KEY_LEN],
    info: &[u8],
    enc: &[u8; KEY_LEN],
    ciphertext: &[u8],
) -> Result<Vec<u8>, Error> {
    if ciphertext.len() < TAG_LEN {
        return Err(Error::Malformed);
    }
    let secret = parse_secret(recipient)?;
    let enc = Encapped::from_bytes(enc).map_err(|_| Error::Malformed)?;
    let mut ctx = setup_receiver::<Aead, Kdf, Suite>(&OpModeR::Base, &secret, &enc, info)
        .map_err(|_| Error::Seal)?;
    ctx.open(ciphertext, b"").map_err(|_| Error::Seal)
}

fn to_key(value: &impl Serializable) -> [u8; KEY_LEN] {
    let bytes = value.to_bytes();
    let slice = bytes.as_slice();
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(slice);
    out
}

fn check_ciphertext(ciphertext: &[u8]) -> Result<(), Error> {
    if ciphertext.len() < TAG_LEN {
        return Err(Error::Malformed);
    }
    Ok(())
}

fn encode_blob(magic: &[u8; 4], enc: &[u8; KEY_LEN], ciphertext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 1 + KEY_LEN + 4 + ciphertext.len());
    out.extend_from_slice(magic);
    out.push(VERSION);
    out.extend_from_slice(enc);
    let len = u32::try_from(ciphertext.len()).expect("ciphertext fits in the frame cap");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(ciphertext);
    out
}

fn decode_blob(magic: &[u8; 4], bytes: &[u8]) -> Result<([u8; KEY_LEN], Vec<u8>), Error> {
    if bytes.len() > MAX_SEALED_LEN {
        return Err(Error::Malformed);
    }
    if bytes.len() < 4 + 1 + KEY_LEN + 4 {
        return Err(Error::Malformed);
    }
    if &bytes[0..4] != magic || bytes[4] != VERSION {
        return Err(Error::Malformed);
    }
    let mut enc = [0u8; KEY_LEN];
    enc.copy_from_slice(&bytes[5..5 + KEY_LEN]);
    let len_at = 5 + KEY_LEN;
    let len = u32::from_le_bytes([
        bytes[len_at],
        bytes[len_at + 1],
        bytes[len_at + 2],
        bytes[len_at + 3],
    ]) as usize;
    let rest = &bytes[len_at + 4..];
    if rest.len() != len {
        return Err(Error::Malformed);
    }
    check_ciphertext(rest)?;
    Ok((enc, rest.to_vec()))
}

impl fmt::Debug for StoreKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("StoreKey")
            .field(&hex_prefix(&self.0))
            .finish()
    }
}

impl fmt::Debug for StoreSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoreSecret([redacted])")
    }
}

impl fmt::Debug for ReplyPublic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ReplyPublic")
            .field(&hex_prefix(&self.0))
            .finish()
    }
}

impl fmt::Debug for ReplySecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReplySecret([redacted])")
    }
}

impl fmt::Debug for SealedLetter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SealedLetter")
            .field("ciphertext_len", &self.ciphertext.len())
            .finish_non_exhaustive()
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes.iter().take(4).fold(String::new(), |mut out, byte| {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
        out
    })
}
