use bitcoin::secp256k1::SecretKey;

use crate::content::ContentRating;
use crate::encode::{self, Reader, Writer};
use crate::payment::DirectPay;
use crate::public_text;
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/store/v1";
const MAGIC: [u8; 4] = *b"OBST";
/// Signed before stores carried a rating. Still decoded and verified.
const VERSION_UNRATED: u8 = 1;
/// Signed before stores carried a mailbox key and a profile. Still decoded and verified.
const VERSION_RATED: u8 = 2;
const VERSION: u8 = 3;

pub const MAX_NAME: usize = 64;
pub const MAX_TAGLINE: usize = 120;
pub const MAX_ABOUT: usize = 2_000;
pub const MAX_POLICY: usize = 1_000;
pub const MAX_PROCESSING: usize = 120;
pub const MAX_MODERATORS: usize = 16;
pub const MAX_SHIPS_TO: usize = 64;
const ONION_HOST_LEN: usize = 56;
/// The last byte of a v3 record: bit 0 Monero, bit 1 Lightning, bit 2
/// Zcash. A record signed before a rail existed has its bit clear and reads
/// the same. A decoder from before a bit refuses the whole record, so a node
/// sets a new bit only once releases that read it are out.
const RAIL_FLAGS: u8 = 0b111;

/// X-only store key. Listings name the store by this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreId([u8; PUBKEY_LEN]);

impl StoreId {
    pub fn from_bytes(bytes: [u8; PUBKEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; PUBKEY_LEN] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        encode::to_hex(&self.0)
    }
}

/// An ISO 3166-1 alpha-2 country code, two uppercase ASCII letters.
///
/// The only shipping geography a store publishes: coarse enough that it does
/// not locate the vendor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CountryCode([u8; 2]);

impl CountryCode {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let bytes: [u8; 2] = text
            .as_bytes()
            .try_into()
            .map_err(|_| Error::BadProfile("a country code is two letters"))?;
        Self::from_bytes(bytes)
    }

    fn from_bytes(bytes: [u8; 2]) -> Result<Self, Error> {
        if !bytes.iter().all(u8::is_ascii_uppercase) {
            return Err(Error::BadProfile("a country code is two uppercase letters"));
        }
        Ok(Self(bytes))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("checked ASCII")
    }
}

/// The public storefront a version 3 store record carries.
///
/// Nothing here locates the vendor: no street, city, coordinates, email, or
/// phone, and the free-text fields are checked by [`public_text::check`]
/// when the record is signed. Images are named by the SHA-256 of their bytes;
/// whoever serves the bytes cannot swap them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreProfile {
    pub tagline: String,
    pub about: String,
    pub avatar: Option<[u8; 32]>,
    pub banner: Option<[u8; 32]>,
    pub shipping_policy: String,
    pub returns_policy: String,
    pub processing_time: String,
    /// Sorted, no repeats.
    pub ships_to: Vec<CountryCode>,
    /// Calendar day the store was created, as YYYYMMDD.
    pub created_on: u32,
}

/// What a version 3 record adds to version 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreDetails {
    /// X25519 key order letters and escrow mail are sealed to. Derived on its
    /// own path, never the store signing scalar.
    pub mailbox_key: [u8; 32],
    /// The vendor's v3 onion host (`<56 base32 chars>.onion`), or empty
    /// while the node has no onion service.
    pub onion: String,
    /// Moderators this store accepts, by the x-only form of each moderator
    /// record's signing key. Sorted, no repeats.
    pub moderators: Vec<[u8; 32]>,
    pub profile: StoreProfile,
    /// The store takes Monero direct pay. Escrow stays Bitcoin-only.
    pub accepts_xmr: bool,
    /// The store's node issues Lightning invoices for direct pay. Escrow
    /// stays on-chain.
    pub accepts_lightning: bool,
    /// The store takes Zcash direct pay, to a shielded address per order.
    /// Escrow stays Bitcoin-only.
    pub accepts_zec: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Store {
    /// Display name.
    pub name: String,
    /// Coarse region the store ships from, such as a state or country.
    pub region: String,
    pub direct_pay: DirectPay,
    pub id: StoreId,
    /// A rating for everything the store sells, on top of each listing's
    /// own label. `None` only for a version 1 record.
    pub rating: Option<ContentRating>,
    /// Mailbox key, onion, moderators, and profile. `None` for a version 1
    /// or 2 record, which a node does not seal letters to.
    pub details: Option<StoreDetails>,
    pub signature: [u8; SIG_LEN],
}

impl Store {
    /// Sign a version 3 store record.
    pub fn sign(
        name: impl Into<String>,
        region: impl Into<String>,
        direct_pay: DirectPay,
        rating: ContentRating,
        details: StoreDetails,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        Self {
            name: name.into(),
            region: region.into(),
            direct_pay,
            id: StoreId::from_bytes(sign::xonly(secret)),
            rating: Some(rating),
            details: Some(details),
            signature: [0u8; SIG_LEN],
        }
        .resign(secret)
    }

    /// Sign this record's fields again with `secret`, which must be the
    /// store's key. Checks the public text and profile rules first.
    pub fn resign(mut self, secret: &SecretKey) -> Result<Self, Error> {
        if sign::xonly(secret) != *self.id.as_bytes() {
            return Err(Error::StoreMismatch);
        }
        self.check_for_signing()?;
        let body = self.encode_body()?;
        self.signature = sign::sign(TAG, &body, secret);
        Ok(self)
    }

    /// The same store with a new rating, signed again with its key.
    pub fn rerate(&self, rating: ContentRating, secret: &SecretKey) -> Result<Self, Error> {
        Self {
            rating: Some(rating),
            ..self.clone()
        }
        .resign(secret)
    }

    /// The record version these fields encode as.
    pub fn version(&self) -> u8 {
        match (&self.rating, &self.details) {
            (None, _) => VERSION_UNRATED,
            (Some(_), None) => VERSION_RATED,
            (Some(_), Some(_)) => VERSION,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut body = self.encode_body()?;
        body.extend_from_slice(&self.signature);
        Ok(body)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (store, len) = Self::decode_one(bytes)?;
        if len != bytes.len() {
            return Err(Error::TrailingBytes);
        }
        Ok(store)
    }

    /// Decode the first store record in `bytes`.
    ///
    /// Returns the record and how many bytes it occupied. Bytes after that
    /// belong to the next catalog record.
    pub(crate) fn decode_one(bytes: &[u8]) -> Result<(Self, usize), Error> {
        let mut reader = Reader::new(bytes);
        let store = Self::decode_body(&mut reader)?;
        let signature = reader.array()?;
        Ok((Self { signature, ..store }, reader.position()))
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(TAG, &body, self.id.as_bytes(), &self.signature)
    }

    fn check_for_signing(&self) -> Result<(), Error> {
        public_text::check("store name", &self.name)?;
        public_text::check_region("store region", &self.region)?;
        let Some(details) = &self.details else {
            return Ok(());
        };
        if self.rating.is_none() {
            return Err(Error::BadProfile("a version 3 store carries a rating"));
        }
        if self.name.trim().is_empty() || self.name.len() > MAX_NAME {
            return Err(Error::BadProfile("the store name is empty or too long"));
        }
        details.check()?;
        let profile = &details.profile;
        for (field, text) in [
            ("tagline", &profile.tagline),
            ("about text", &profile.about),
            ("shipping policy", &profile.shipping_policy),
            ("returns policy", &profile.returns_policy),
            ("processing time", &profile.processing_time),
        ] {
            public_text::check(field, text)?;
        }
        Ok(())
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(self.version());
        writer.str(&self.name)?;
        writer.str(&self.region)?;
        writer.str(&self.direct_pay.bolt12_offer)?;
        writer.str(&self.direct_pay.silent_payment_code)?;
        writer.bytes(self.id.as_bytes());
        if let Some(rating) = self.rating {
            writer.u8(rating.tag());
        }
        if let Some(details) = &self.details {
            details.check()?;
            details.encode(&mut writer)?;
        }
        Ok(writer.finish())
    }

    fn decode_body(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let version = reader.expect_magic_in(&MAGIC, &[VERSION_UNRATED, VERSION_RATED, VERSION])?;
        let mut store = Self {
            name: reader.str()?,
            region: reader.str()?,
            direct_pay: DirectPay {
                bolt12_offer: reader.str()?,
                silent_payment_code: reader.str()?,
            },
            id: StoreId::from_bytes(reader.array()?),
            rating: None,
            details: None,
            signature: [0u8; SIG_LEN],
        };
        if version >= VERSION_RATED {
            store.rating = Some(ContentRating::from_tag(reader.u8()?)?);
        }
        if version == VERSION {
            let details = StoreDetails::decode(reader)?;
            details.check()?;
            store.details = Some(details);
        }
        Ok(store)
    }
}

impl StoreDetails {
    /// Length, order, and shape rules. Both encoding and decoding enforce
    /// them, so a record has one encoding.
    fn check(&self) -> Result<(), Error> {
        if !self.onion.is_empty() && !is_onion_host(&self.onion) {
            return Err(Error::BadProfile("the onion is not a v3 onion host"));
        }
        if self.moderators.len() > MAX_MODERATORS {
            return Err(Error::BadProfile("too many moderators"));
        }
        if !self.moderators.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(Error::BadProfile("moderators are not sorted or repeat"));
        }
        let profile = &self.profile;
        for (text, max, what) in [
            (&profile.tagline, MAX_TAGLINE, "the tagline is too long"),
            (&profile.about, MAX_ABOUT, "the about text is too long"),
            (&profile.shipping_policy, MAX_POLICY, "the shipping policy is too long"),
            (&profile.returns_policy, MAX_POLICY, "the returns policy is too long"),
            (&profile.processing_time, MAX_PROCESSING, "the processing time is too long"),
        ] {
            if text.len() > max {
                return Err(Error::BadProfile(what));
            }
        }
        if profile.ships_to.len() > MAX_SHIPS_TO {
            return Err(Error::BadProfile("too many ship-to countries"));
        }
        if !profile.ships_to.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(Error::BadProfile("ship-to countries are not sorted or repeat"));
        }
        if !is_date(profile.created_on) {
            return Err(Error::BadProfile("the created-on date is not YYYYMMDD"));
        }
        Ok(())
    }

    fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        let profile = &self.profile;
        writer.bytes(&self.mailbox_key);
        writer.str(&self.onion)?;
        writer.u8(self.moderators.len() as u8);
        for moderator in &self.moderators {
            writer.bytes(moderator);
        }
        writer.str(&profile.tagline)?;
        writer.str(&profile.about)?;
        write_hash(writer, profile.avatar);
        write_hash(writer, profile.banner);
        writer.str(&profile.shipping_policy)?;
        writer.str(&profile.returns_policy)?;
        writer.str(&profile.processing_time)?;
        writer.u8(profile.ships_to.len() as u8);
        for country in &profile.ships_to {
            writer.bytes(&country.0);
        }
        writer.u32(profile.created_on);
        writer.u8(u8::from(self.accepts_xmr) | u8::from(self.accepts_lightning) << 1 | u8::from(self.accepts_zec) << 2);
        Ok(())
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let mailbox_key = reader.array()?;
        let onion = reader.str()?;
        let count = reader.u8()? as usize;
        if count > MAX_MODERATORS {
            return Err(Error::BadProfile("too many moderators"));
        }
        let mut moderators = Vec::with_capacity(count);
        for _ in 0..count {
            moderators.push(reader.array()?);
        }
        let tagline = reader.str()?;
        let about = reader.str()?;
        let avatar = read_hash(reader)?;
        let banner = read_hash(reader)?;
        let shipping_policy = reader.str()?;
        let returns_policy = reader.str()?;
        let processing_time = reader.str()?;
        let count = reader.u8()? as usize;
        if count > MAX_SHIPS_TO {
            return Err(Error::BadProfile("too many ship-to countries"));
        }
        let mut ships_to = Vec::with_capacity(count);
        for _ in 0..count {
            ships_to.push(CountryCode::from_bytes(reader.array()?)?);
        }
        let created_on = reader.u32()?;
        let rails = reader.u8()?;
        if rails > RAIL_FLAGS {
            return Err(Error::BadProfile("an unknown payment rail flag is set"));
        }
        Ok(Self {
            mailbox_key,
            onion,
            moderators,
            profile: StoreProfile {
                tagline,
                about,
                avatar,
                banner,
                shipping_policy,
                returns_policy,
                processing_time,
                ships_to,
                created_on,
            },
            accepts_xmr: rails & 1 != 0,
            accepts_lightning: rails & 2 != 0,
            accepts_zec: rails & 4 != 0,
        })
    }
}

fn write_hash(writer: &mut Writer, hash: Option<[u8; 32]>) {
    match hash {
        None => writer.u8(0),
        Some(hash) => {
            writer.u8(1);
            writer.bytes(&hash);
        }
    }
}

fn read_hash(reader: &mut Reader<'_>) -> Result<Option<[u8; 32]>, Error> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(reader.array()?)),
        _ => Err(Error::BadProfile("an image flag is 0 or 1")),
    }
}

fn is_onion_host(text: &str) -> bool {
    text.strip_suffix(".onion").is_some_and(|host| {
        host.len() == ONION_HOST_LEN && host.bytes().all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
    })
}

fn is_date(value: u32) -> bool {
    let (year, month, day) = (value / 10_000, value / 100 % 100, value % 100);
    (1970..=9999).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pay() -> DirectPay {
        DirectPay {
            bolt12_offer: "bolt12-offer:fixture:marlow-goods".to_string(),
            silent_payment_code: "bip352-code:fixture:marlow-goods".to_string(),
        }
    }

    fn details() -> StoreDetails {
        StoreDetails {
            mailbox_key: [9u8; 32],
            onion: format!("{}.onion", "a".repeat(56)),
            moderators: vec![[1u8; 32], [2u8; 32]],
            profile: StoreProfile {
                tagline: "Books and wool from the coast".to_string(),
                about: "A two-person shop. We pack everything flat.".to_string(),
                avatar: Some([3u8; 32]),
                banner: None,
                shipping_policy: "Ships within three days.".to_string(),
                returns_policy: "Returns within 30 days.".to_string(),
                processing_time: "1 to 3 days".to_string(),
                ships_to: vec![CountryCode::parse("CA").unwrap(), CountryCode::parse("US").unwrap()],
                created_on: 20260101,
            },
            accepts_xmr: true,
            accepts_lightning: true,
            accepts_zec: true,
        }
    }

    fn sample() -> Store {
        Store::sign("Marlow Goods", "Oregon", pay(), ContentRating::General, details(), &sign::secret_from_label("store-test")).unwrap()
    }

    /// A version 2 record: rated, no details.
    fn rated() -> Store {
        Store {
            details: None,
            ..sample()
        }
        .resign(&sign::secret_from_label("store-test"))
        .unwrap()
    }

    /// The fixture store as the version 1 code signed it, before ratings.
    const UNRATED_STORE: &str = "4f425354010c004d61726c6f7720476f6f647306004f7265676f6e2100626f6c7431322d6f666665723a666978747572653a6d61726c6f772d676f6f647320006269703335322d636f64653a666978747572653a6d61726c6f772d676f6f64732b489bf1506ec6e9fc3d968a519fcaf07ffe0394fdc313b513fc7d0ba4229056a70d3658e80be6a04bf99694daf664f22842d0860d82fb274a245ed59811a8ffb07c5e61ec90f020f02d698b161f5260dd9cfe8fb440d17da686d44c7b75891f";

    fn from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn version_1_store_still_verifies_as_unrated() {
        let bytes = from_hex(UNRATED_STORE);
        let store = Store::decode(&bytes).unwrap();
        assert_eq!((store.rating, store.version()), (None, 1));
        assert!(store.details.is_none());
        store.verify().unwrap();
        assert_eq!(store.encode().unwrap(), bytes);
    }

    #[test]
    fn version_2_store_still_decodes_and_verifies() {
        let store = rated();
        let bytes = store.encode().unwrap();
        assert_eq!(bytes[4], 2);
        let decoded = Store::decode(&bytes).unwrap();
        assert_eq!(decoded, store);
        assert!(decoded.details.is_none());
        decoded.verify().unwrap();
    }

    #[test]
    fn version_3_round_trips_with_mailbox_moderators_and_profile() {
        let store = sample();
        let bytes = store.encode().unwrap();
        assert_eq!(bytes[4], 3);
        let decoded = Store::decode(&bytes).unwrap();
        assert_eq!(decoded, store);
        decoded.verify().unwrap();
        let details = decoded.details.unwrap();
        assert_eq!(details.mailbox_key, [9u8; 32]);
        assert_eq!(details.profile.ships_to[1].as_str(), "US");
    }

    #[test]
    fn every_version_3_field_is_covered_by_the_signature() {
        let store = sample();
        let tampers: Vec<Box<dyn Fn(&mut StoreDetails)>> = vec![
            Box::new(|d| d.mailbox_key[0] ^= 1),
            Box::new(|d| d.onion.clear()),
            Box::new(|d| d.moderators.pop().map(|_| ()).unwrap()),
            Box::new(|d| d.profile.tagline.push('!')),
            Box::new(|d| d.profile.about.push('!')),
            Box::new(|d| d.profile.avatar = None),
            Box::new(|d| d.profile.banner = Some([4u8; 32])),
            Box::new(|d| d.profile.shipping_policy.push('!')),
            Box::new(|d| d.profile.returns_policy.push('!')),
            Box::new(|d| d.profile.processing_time.push('!')),
            Box::new(|d| d.profile.ships_to.truncate(1)),
            Box::new(|d| d.profile.created_on += 1),
            Box::new(|d| d.accepts_xmr = !d.accepts_xmr),
            Box::new(|d| d.accepts_lightning = !d.accepts_lightning),
            Box::new(|d| d.accepts_zec = !d.accepts_zec),
        ];
        for tamper in tampers {
            let mut changed = store.clone();
            tamper(changed.details.as_mut().unwrap());
            assert_eq!(changed.verify(), Err(Error::BadSignature));
        }
        let mut stripped = store;
        stripped.details = None;
        assert_eq!(stripped.verify(), Err(Error::BadSignature), "a v3 record cannot be downgraded to v2");
    }

    #[test]
    fn non_canonical_details_are_refused() {
        let secret = sign::secret_from_label("store-test");
        let with = |change: &dyn Fn(&mut StoreDetails)| {
            let mut d = details();
            change(&mut d);
            Store::sign("Marlow Goods", "Oregon", pay(), ContentRating::General, d, &secret)
        };
        assert!(matches!(with(&|d| d.moderators.reverse()), Err(Error::BadProfile(_))));
        assert!(matches!(with(&|d| d.moderators.push([2u8; 32])), Err(Error::BadProfile(_))));
        assert!(matches!(with(&|d| d.profile.ships_to.reverse()), Err(Error::BadProfile(_))));
        assert!(matches!(with(&|d| d.onion = "example.com".into()), Err(Error::BadProfile(_))));
        assert!(matches!(with(&|d| d.profile.created_on = 20261340), Err(Error::BadProfile(_))));
        assert!(matches!(with(&|d| d.profile.tagline = "x".repeat(MAX_TAGLINE + 1)), Err(Error::BadProfile(_))));
        assert!(CountryCode::parse("us").is_err() && CountryCode::parse("USA").is_err());

        let mut bytes = sample().encode().unwrap();
        let flag = bytes.len() - 64 - 1 - 4 - 1 - 2 * 2 - 2 - 11 - 2 - 23 - 2 - 24 - 1 - 33;
        assert_eq!(bytes[flag], 1, "avatar flag");
        bytes[flag] = 2;
        assert!(matches!(Store::decode(&bytes), Err(Error::BadProfile(_))));

        let mut bytes = sample().encode().unwrap();
        let rails = bytes.len() - 64 - 1;
        assert_eq!(bytes[rails], 7, "Monero, Lightning, and Zcash flags");
        bytes[rails] = 8;
        assert!(matches!(Store::decode(&bytes), Err(Error::BadProfile(_))));
        bytes[rails] = 1;
        let monero_only = Store::decode(&bytes).unwrap();
        let details = monero_only.details.as_ref().unwrap();
        assert!(details.accepts_xmr && !details.accepts_lightning && !details.accepts_zec);
        assert!(monero_only.verify().is_err(), "dropping the Lightning and Zcash flags kept the signature valid");
        bytes[rails] = 4;
        let zcash_only = Store::decode(&bytes).unwrap().details.unwrap();
        assert!(zcash_only.accepts_zec && !zcash_only.accepts_xmr && !zcash_only.accepts_lightning);
    }

    #[test]
    fn a_record_signed_before_zcash_reads_the_same() {
        let secret = sign::secret_from_label("store-test");
        let mut before = details();
        before.accepts_zec = false;
        let store = Store::sign("Marlow Goods", "Oregon", pay(), ContentRating::General, before, &secret).unwrap();
        let bytes = store.encode().unwrap();
        assert_eq!(bytes[bytes.len() - 64 - 1], 3, "the byte a Monero and Lightning store signed");
        let read = Store::decode(&bytes).unwrap();
        read.verify().unwrap();
        assert!(!read.details.unwrap().accepts_zec);
    }

    #[test]
    fn signing_refuses_contact_or_location_text() {
        let secret = sign::secret_from_label("store-test");
        let mut d = details();
        d.profile.about = "Visit us at 12 Cedar Street.".into();
        assert_eq!(
            Store::sign("Marlow Goods", "Oregon", pay(), ContentRating::General, d, &secret),
            Err(Error::PrivateText { field: "about text", kind: public_text::PrivateText::StreetAddress })
        );
        let mut d = details();
        d.profile.returns_policy = "Email returns@marlow.example first.".into();
        assert!(matches!(
            Store::sign("Marlow Goods", "Oregon", pay(), ContentRating::General, d, &secret),
            Err(Error::PrivateText { field: "returns policy", .. })
        ));
        assert!(matches!(
            Store::sign("Call 503 555 0142", "Oregon", pay(), ContentRating::General, details(), &secret),
            Err(Error::PrivateText { field: "store name", .. })
        ));
    }

    #[test]
    fn rating_is_covered_by_the_signature() {
        let store = sample().rerate(ContentRating::Adult, &sign::secret_from_label("store-test")).unwrap();
        let decoded = Store::decode(&store.encode().unwrap()).unwrap();
        assert_eq!(decoded.rating, Some(ContentRating::Adult));
        decoded.verify().unwrap();

        let mut softened = store.clone();
        softened.rating = Some(ContentRating::General);
        assert_eq!(softened.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn rerate_signs_again_with_the_store_key_only() {
        let store = sample();
        let adult = store.rerate(ContentRating::Adult, &sign::secret_from_label("store-test")).unwrap();
        assert_eq!(adult.rating, Some(ContentRating::Adult));
        assert_eq!((adult.id, &adult.name, &adult.details), (store.id, &store.name, &store.details));
        adult.verify().unwrap();
        assert_eq!(
            store.rerate(ContentRating::Adult, &sign::secret_from_label("someone-else")),
            Err(Error::StoreMismatch)
        );
    }

    #[test]
    fn tampered_name_or_signature_is_rejected() {
        let mut store = sample();
        store.name.push('!');
        assert_eq!(store.verify(), Err(Error::BadSignature));
        let mut store = sample();
        store.signature[63] ^= 0x01;
        assert_eq!(store.verify(), Err(Error::BadSignature));
    }
}
