//! TESTNET4 DEMO DATA: three sample stores and their public fixture keys.
//!
//! Every secret here is derived from a public label, so anyone can rebuild
//! the store and mailbox keys from this source. The stores exist so that the
//! demo catalog has real signatures and real letters; a node serves them
//! only on testnet4. A node's own store is signed with the key from its
//! backup phrase, not with anything in this file.

use bitcoin::secp256k1::SecretKey;

use crate::content::{ContentLabel, ContentRating};
use crate::dispute::{DisputeEnvelope, DisputePlaintext};
use crate::encode;
use crate::escrow::{EscrowBranch, EscrowTerms};
use crate::listing::{Listing, ListingKind};
use crate::order::{OrderEnvelope, OrderPlaintext};
use crate::payment::DirectPay;
use crate::sign;
use crate::store::{CountryCode, Store, StoreDetails, StoreId, StoreProfile};
use crate::Error;

/// Marlow Goods's store key. The label predates the other stores, so records
/// signed under it in earlier versions keep their keys.
const STORE_KEY: &str = "openbazaar/phase0/fixture/store";
const VENDOR_SPEND_KEY: &str = "openbazaar/phase0/fixture/vendor-spend";
const SCAN_KEY: &str = "openbazaar/phase0/fixture/scan";
const BUYER_SPEND_KEY: &str = "openbazaar/phase0/fixture/buyer-spend";
const MODERATOR_KEY: &str = "openbazaar/phase0/fixture/moderator";
const ADAPTOR_POINT: &str = "openbazaar/phase0/fixture/adaptor-point";
const VENDOR_WINS_POINT: &str = "openbazaar/phase0/fixture/vendor-wins-point";

fn compressed(label: &str) -> [u8; 33] {
    let secp = bitcoin::secp256k1::Secp256k1::signing_only();
    bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &sign::secret_from_label(label))
        .serialize()
}

struct Draft {
    title: &'static str,
    category: &'static str,
    price_sats: u64,
    ship_from: &'static str,
    description: &'static str,
    seller: &'static str,
    stock: u32,
    listed_on: u32,
    moderated: bool,
    kind: ListingKind,
}

const LISTINGS: &[Draft] = &[
    Draft { title: "Paperback novel", category: "Books", price_sats: 18_000, ship_from: "Oregon", description: "A sewn paperback from a small press. It is packed flat so the spine stays straight.", seller: "Marlow Goods", stock: 8, listed_on: 20260312, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Field guide to tide pools", category: "Books", price_sats: 12_500, ship_from: "Oregon", description: "A pocket field guide to the Oregon coast, with drawings of anemones, crabs, and kelp.", seller: "Marlow Goods", stock: 14, listed_on: 20260202, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Clothbound notebook", category: "Paper", price_sats: 6_400, ship_from: "Oregon", description: "A clothbound notebook with blank laid paper, thread sewn. It fits in a coat pocket.", seller: "Marlow Goods", stock: 2, listed_on: 20261020, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Field recordings", category: "Music", price_sats: 4_000, ship_from: "Digital", description: "Four tracks: rain on a metal roof, a freight line, a night market, and a quiet street. Delivered as files.", seller: "Marlow Goods", stock: 40, listed_on: 20260115, moderated: true, kind: ListingKind::Digital },
    Draft { title: "Piano etudes", category: "Music", price_sats: 3_500, ship_from: "Digital", description: "Twelve short etudes for late-intermediate piano, in one PDF.", seller: "Marlow Goods", stock: 40, listed_on: 20260401, moderated: false, kind: ListingKind::Digital },
    Draft { title: "Wool socks", category: "Clothing", price_sats: 9_000, ship_from: "Oregon", description: "A pair of crew-length wool socks, with nylon at the heel. Sized for a medium adult foot.", seller: "Marlow Goods", stock: 6, listed_on: 20260518, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Work shirt", category: "Clothing", price_sats: 22_000, ship_from: "Oregon", description: "A heavy cotton work shirt with one chest pocket. Natural color, cut to fit a range of bodies.", seller: "Marlow Goods", stock: 4, listed_on: 20260709, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Linen dish towel", category: "Home", price_sats: 7_000, ship_from: "Oregon", description: "Undyed linen, hemmed, about 20 by 28 inches. Washed once so it is ready to dry dishes.", seller: "Marlow Goods", stock: 11, listed_on: 20260328, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Wool blanket, twin", category: "Home", price_sats: 42_000, ship_from: "Oregon", description: "A twin-size blanket in undyed wool. It is heavy enough for a cool room.", seller: "Marlow Goods", stock: 3, listed_on: 20260814, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Beeswax candles, set of 4", category: "Home", price_sats: 5_600, ship_from: "Oregon", description: "Four short beeswax tapers. Each burns for about five hours.", seller: "Marlow Goods", stock: 9, listed_on: 20260220, moderated: false, kind: ListingKind::Physical },
    Draft { title: "Stoneware mug", category: "Home", price_sats: 11_000, ship_from: "Oregon", description: "A 12 ounce stoneware mug with a dull white glaze. It is dishwasher safe.", seller: "Marlow Goods", stock: 7, listed_on: 20260901, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Wool hat pattern", category: "Paper", price_sats: 2_400, ship_from: "Digital", description: "A written pattern for an adult wool hat, in one PDF.", seller: "Marlow Goods", stock: 30, listed_on: 20260502, moderated: false, kind: ListingKind::Digital },
    Draft { title: "Letterpress cards", category: "Paper", price_sats: 4_800, ship_from: "Vermont", description: "Eight blank folded cards with a debossed rule, plus envelopes.", seller: "Hale Paper", stock: 15, listed_on: 20260422, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Walnut ink, 30 ml", category: "Paper", price_sats: 5_200, ship_from: "Vermont", description: "Thirty milliliters of walnut ink for a dip pen. The color is brown.", seller: "Hale Paper", stock: 5, listed_on: 20260303, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Cotton paper, ream", category: "Paper", price_sats: 16_000, ship_from: "Vermont", description: "Five hundred sheets of cotton paper, 90 gsm, cut for letters.", seller: "Hale Paper", stock: 2, listed_on: 20261105, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Dip pen", category: "Tools", price_sats: 7_800, ship_from: "Vermont", description: "A steel nib and a short wood holder. It ships in a paper sleeve.", seller: "Hale Paper", stock: 6, listed_on: 20260611, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Sewing shears", category: "Tools", price_sats: 14_000, ship_from: "Vermont", description: "Eight-inch sewing shears for cloth. They arrive in a paper sleeve.", seller: "Hale Paper", stock: 4, listed_on: 20260128, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Garden trowel", category: "Tools", price_sats: 6_500, ship_from: "Vermont", description: "A stainless garden trowel with a short wood handle. For pots and garden beds.", seller: "Hale Paper", stock: 8, listed_on: 20260722, moderated: false, kind: ListingKind::Physical },
    Draft { title: "Linen tape", category: "Tools", price_sats: 3_200, ship_from: "Vermont", description: "A 150 centimeter linen tape with metal tips, for cloth and paper.", seller: "Hale Paper", stock: 20, listed_on: 20260830, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Graphite pencils", category: "Paper", price_sats: 1_800, ship_from: "Vermont", description: "A dozen cedar pencils, HB, unsharpened, in a paper band.", seller: "Hale Paper", stock: 25, listed_on: 20260214, moderated: false, kind: ListingKind::Physical },
    Draft { title: "Ink blotter", category: "Paper", price_sats: 2_200, ship_from: "Vermont", description: "A rocker blotter with a strip of blotting paper. The paper can be replaced.", seller: "Hale Paper", stock: 12, listed_on: 20260918, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Binding thread", category: "Tools", price_sats: 2_600, ship_from: "Vermont", description: "Linen bookbinding thread, 25 meters, unwaxed.", seller: "Hale Paper", stock: 18, listed_on: 20260525, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Sketchbook", category: "Paper", price_sats: 9_000, ship_from: "Vermont", description: "A sewn sketchbook of drawing paper, 32 pages, no printed lines.", seller: "Hale Paper", stock: 3, listed_on: 20261002, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Printmaking brayer", category: "Tools", price_sats: 19_000, ship_from: "Vermont", description: "A 10 centimeter rubber brayer with a metal frame, for ink on paper.", seller: "Hale Paper", stock: 2, listed_on: 20260408, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Whole bean coffee, 12 oz", category: "Grocery", price_sats: 9_500, ship_from: "Maine", description: "Washed-process beans, roasted weekly in a 12 ounce bag. Ships as whole beans.", seller: "North Kitchen", stock: 16, listed_on: 20260319, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Wildflower honey, 8 oz", category: "Grocery", price_sats: 8_200, ship_from: "Maine", description: "Raw wildflower honey in an 8 ounce glass jar. The lid is taped for the trip.", seller: "North Kitchen", stock: 10, listed_on: 20260602, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Stone-ground oats, 2 lb", category: "Grocery", price_sats: 4_800, ship_from: "Maine", description: "Thick stone-ground oats in a 2 pound paper sack.", seller: "North Kitchen", stock: 22, listed_on: 20260108, moderated: false, kind: ListingKind::Physical },
    Draft { title: "Weeknight recipes", category: "Grocery", price_sats: 1_800, ship_from: "Digital", description: "Twenty weeknight recipes in one PDF. The ingredients are ordinary groceries.", seller: "North Kitchen", stock: 50, listed_on: 20260715, moderated: false, kind: ListingKind::Digital },
    Draft { title: "Maple syrup, 8 oz", category: "Grocery", price_sats: 14_000, ship_from: "Maine", description: "Grade A maple syrup in an 8 ounce glass bottle. The cap is sealed.", seller: "North Kitchen", stock: 6, listed_on: 20260228, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Sea salt", category: "Grocery", price_sats: 3_600, ship_from: "Maine", description: "Flake sea salt in a 4 ounce tin. It is for the table, not for curing.", seller: "North Kitchen", stock: 30, listed_on: 20260808, moderated: false, kind: ListingKind::Physical },
    Draft { title: "Dried kelp", category: "Grocery", price_sats: 5_400, ship_from: "Maine", description: "Dried kelp fronds in a paper pouch, about 1 ounce, for broth.", seller: "North Kitchen", stock: 9, listed_on: 20260925, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Cast iron trivet", category: "Home", price_sats: 15_000, ship_from: "Maine", description: "A round cast iron trivet, about 7 inches across, for a hot pot.", seller: "North Kitchen", stock: 4, listed_on: 20260506, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Cotton apron", category: "Clothing", price_sats: 18_000, ship_from: "Maine", description: "A natural cotton apron with a waist tie and one pocket.", seller: "North Kitchen", stock: 5, listed_on: 20261012, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Wool mittens", category: "Clothing", price_sats: 12_000, ship_from: "Maine", description: "A pair of boiled wool mittens, sized for an adult hand.", seller: "North Kitchen", stock: 7, listed_on: 20261102, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Ceramic pourer", category: "Home", price_sats: 13_000, ship_from: "Maine", description: "A small ceramic pourer for oil or syrup, glazed white inside.", seller: "North Kitchen", stock: 3, listed_on: 20260416, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Kitchen shears", category: "Tools", price_sats: 8_500, ship_from: "Maine", description: "Kitchen shears that come apart for washing. The handles are steel.", seller: "North Kitchen", stock: 8, listed_on: 20260307, moderated: false, kind: ListingKind::Physical },
    Draft { title: "Wool socks", category: "Clothing", price_sats: 8_500, ship_from: "Vermont", description: "A pair of crew-length wool socks, dyed a quiet grey. Sized for a medium adult foot.", seller: "Hale Paper", stock: 4, listed_on: 20261028, moderated: true, kind: ListingKind::Physical },
    Draft { title: "Beeswax candles, set of 4", category: "Home", price_sats: 5_200, ship_from: "Maine", description: "Four short beeswax tapers from a Maine apiary. Each burns for about five hours.", seller: "North Kitchen", stock: 6, listed_on: 20261101, moderated: true, kind: ListingKind::Physical },
];

fn product_key(title: &str) -> &'static str {
    match title {
        "Wool socks" => "wool-socks",
        "Beeswax candles, set of 4" => "beeswax-candles",
        _ => "",
    }
}

/// Flat package shipping and the item subtotal that waives it.
///
/// Digital goods do not ship. A physical package uses one rate for the seller.
pub fn package_rate(seller: &str, kind: ListingKind) -> (u64, u64) {
    if kind == ListingKind::Digital {
        return (0, 0);
    }
    match seller {
        "Hale Paper" => (600, 20_000),
        "North Kitchen" => (900, 25_000),
        _ => (800, 30_000),
    }
}

/// One demo store: its keys and its public profile.
struct FixtureStore {
    name: &'static str,
    region: &'static str,
    slug: &'static str,
    store_key: &'static str,
    mail_key: &'static str,
    tagline: &'static str,
    about: &'static str,
    shipping_policy: &'static str,
    returns_policy: &'static str,
    processing_time: &'static str,
    ships_to: &'static [&'static str],
    created_on: u32,
    avatar: &'static [u8],
    banner: &'static [u8],
}

const STORES: &[FixtureStore] = &[
    FixtureStore {
        name: "Marlow Goods",
        region: "Oregon",
        slug: "marlow-goods",
        store_key: STORE_KEY,
        mail_key: "openbazaar/testnet4/fixture/mail/marlow",
        tagline: "Books, wool, and kitchen things from the Oregon coast.",
        about: "A small shop of things we use ourselves: sewn paperbacks, wool socks and blankets, linen, and plain stoneware. Everything is packed flat in paper.",
        shipping_policy: "Ships in recycled paper. One flat rate per package; free over 30,000 sats.",
        returns_policy: "Unused goods can come back within 30 days. Digital goods are final.",
        processing_time: "1 to 3 days",
        ships_to: &["CA", "US"],
        created_on: 20260105,
        avatar: include_bytes!("../fixtures/media/marlow-goods-avatar.png"),
        banner: include_bytes!("../fixtures/media/marlow-goods-banner.png"),
    },
    FixtureStore {
        name: "Hale Paper",
        region: "Vermont",
        slug: "hale-paper",
        store_key: "openbazaar/testnet4/fixture/store/hale-paper",
        mail_key: "openbazaar/phase0/fixture/mail/hale",
        tagline: "Paper, ink, and small tools for people who write by hand.",
        about: "Cotton paper, walnut ink, dip pens, and binding thread, chosen by a bookbinder. Tools ship sleeved in paper.",
        shipping_policy: "One flat rate per package; free over 20,000 sats.",
        returns_policy: "Returns within 14 days if unopened.",
        processing_time: "2 to 4 days",
        ships_to: &["US"],
        created_on: 20260120,
        avatar: include_bytes!("../fixtures/media/hale-paper-avatar.png"),
        banner: include_bytes!("../fixtures/media/hale-paper-banner.png"),
    },
    FixtureStore {
        name: "North Kitchen",
        region: "Maine",
        slug: "north-kitchen",
        store_key: "openbazaar/testnet4/fixture/store/north-kitchen",
        mail_key: "openbazaar/phase0/fixture/mail/north",
        tagline: "Coffee, honey, oats, and syrup, packed the week they ship.",
        about: "Pantry goods from small producers. Coffee is roasted weekly, and jars are taped for the trip.",
        shipping_policy: "One flat rate per package; free over 25,000 sats.",
        returns_policy: "Food cannot be returned. Tell us if a jar arrives broken.",
        processing_time: "Ships within a week",
        ships_to: &["CA", "US"],
        created_on: 20260201,
        avatar: include_bytes!("../fixtures/media/north-kitchen-avatar.png"),
        banner: include_bytes!("../fixtures/media/north-kitchen-banner.png"),
    },
];

/// A demo store's secrets. Public; testnet4 only.
#[derive(Clone)]
pub struct FixtureStoreKeys {
    /// Signs the store record and its listings.
    pub store: SecretKey,
    /// X25519 mailbox secret, on its own label, not the store scalar.
    pub mailbox: [u8; 32],
}

/// The keys of the demo store named `name`.
pub fn fixture_store_keys(name: &str) -> Option<FixtureStoreKeys> {
    let store = STORES.iter().find(|store| store.name == name)?;
    Some(FixtureStoreKeys {
        store: sign::secret_from_label(store.store_key),
        mailbox: sign::content_hash(store.mail_key.as_bytes()),
    })
}

/// The demo stores' avatar and banner images, as served bytes.
pub fn fixture_media() -> Vec<&'static [u8]> {
    STORES.iter().flat_map(|store| [store.avatar, store.banner]).collect()
}

fn fixture_store(draft: &FixtureStore, moderator: [u8; 32]) -> Store {
    let keys = fixture_store_keys(draft.name).expect("fixture store has keys");
    let mailbox_key = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(keys.mailbox)).to_bytes();
    let details = StoreDetails {
        mailbox_key,
        onion: String::new(),
        moderators: vec![moderator],
        profile: StoreProfile {
            tagline: draft.tagline.to_string(),
            about: draft.about.to_string(),
            avatar: Some(sign::content_hash(draft.avatar)),
            banner: Some(sign::content_hash(draft.banner)),
            shipping_policy: draft.shipping_policy.to_string(),
            returns_policy: draft.returns_policy.to_string(),
            processing_time: draft.processing_time.to_string(),
            ships_to: draft
                .ships_to
                .iter()
                .map(|code| CountryCode::parse(code).expect("fixture country code"))
                .collect(),
            created_on: draft.created_on,
        },
        accepts_xmr: false,
    };
    Store::sign(
        draft.name,
        draft.region,
        DirectPay {
            bolt12_offer: format!("bolt12-offer:fixture:{}", draft.slug),
            silent_payment_code: format!("bip352-code:fixture:{}", draft.slug),
        },
        ContentRating::General,
        details,
        &keys.store,
    )
    .expect("fixture store signs")
}

/// Fixture spend, scan, and review keys. These are not a wallet.
///
/// The direct-pay strings on [`fixture_catalog`] are placeholders. The node
/// replaces them with BOLT 12 and BIP 352 values and re-signs the store.
#[derive(Clone)]
pub struct FixtureKeys {
    pub spend: SecretKey,
    pub scan: SecretKey,
    pub buyer: SecretKey,
    pub moderator: SecretKey,
}

/// A person who can be asked to open a dispute. Not a script leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moderator {
    pub name: String,
    pub id: [u8; 32],
}

impl Moderator {
    pub fn id_hex(&self) -> String {
        encode::to_hex(&self.id)
    }
}

pub fn fixture_keys() -> FixtureKeys {
    FixtureKeys {
        spend: sign::secret_from_label(VENDOR_SPEND_KEY),
        scan: sign::secret_from_label(SCAN_KEY),
        buyer: sign::secret_from_label(BUYER_SPEND_KEY),
        moderator: sign::secret_from_label(MODERATOR_KEY),
    }
}

/// Store records and the listings they signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    /// Each listing's `store_id` names one of these. Ids do not repeat.
    pub stores: Vec<Store>,
    /// Ordinary reviewer for moderated checkout. Every fixture listing accepts them.
    pub moderator: Moderator,
    pub listings: Vec<Listing>,
}

impl Catalog {
    pub fn verify(&self) -> Result<(), Error> {
        for (i, store) in self.stores.iter().enumerate() {
            store.verify()?;
            if self.stores[..i].iter().any(|other| other.id == store.id) {
                return Err(Error::StoreMismatch);
            }
        }
        for listing in &self.listings {
            listing.verify()?;
            if self.store(&listing.store_id).is_none() {
                return Err(Error::StoreMismatch);
            }
        }
        Ok(())
    }

    pub fn store(&self, id: &StoreId) -> Option<&Store> {
        self.stores.iter().find(|store| store.id == *id)
    }

    /// The store that signed `listing`.
    pub fn store_of(&self, listing: &Listing) -> Option<&Store> {
        self.store(&listing.store_id)
    }

    /// Store records first, then listings.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        for store in &self.stores {
            bytes.extend(store.encode()?);
        }
        for listing in &self.listings {
            bytes.extend(listing.encode()?);
        }
        Ok(bytes)
    }

    /// Decode one or more store records followed by listing records.
    ///
    /// `moderator` is not inside the signed bytes. Call [`Catalog::verify`]
    /// after this; a bad signature returns [`Error::BadSignature`].
    pub fn decode_signed(bytes: &[u8], moderator: Moderator) -> Result<Self, Error> {
        let mut stores = Vec::new();
        let mut offset = 0;
        while bytes[offset..].starts_with(b"OBST") || stores.is_empty() {
            let (store, len) = crate::store::Store::decode_one(&bytes[offset..])?;
            offset += len;
            stores.push(store);
        }
        let mut listings = Vec::new();
        while offset < bytes.len() {
            let (listing, len) = crate::listing::Listing::decode_one(&bytes[offset..])?;
            offset += len;
            listings.push(listing);
        }
        Ok(Self {
            stores,
            moderator,
            listings,
        })
    }
}

/// The three demo stores and their listings, each listing signed by its own store.
pub fn fixture_catalog() -> Catalog {
    let keys = fixture_keys();
    let moderator = Moderator {
        name: "June Pell".to_string(),
        id: sign::xonly(&keys.moderator),
    };
    let stores: Vec<Store> = STORES.iter().map(|draft| fixture_store(draft, moderator.id)).collect();
    let listings = LISTINGS
        .iter()
        .map(|draft| {
            let store = stores.iter().find(|store| store.name == draft.seller).expect("listing names a fixture store");
            let secret = fixture_store_keys(draft.seller).expect("fixture store has keys").store;
            let (shipping_sats, free_over_sats) = package_rate(draft.seller, draft.kind);
            Listing::sign(
                store.id,
                draft.title,
                draft.category,
                draft.price_sats,
                draft.ship_from,
                draft.description,
                draft.seller,
                draft.stock,
                draft.listed_on,
                draft.moderated,
                draft.kind,
                product_key(draft.title),
                shipping_sats,
                free_over_sats,
                ContentLabel::GENERAL,
                &secret,
            )
            .expect("fixture listing is signed by its fixture store")
        })
        .collect();
    Catalog {
        stores,
        moderator,
        listings,
    }
}

pub fn fixture_order(catalog: &Catalog) -> OrderEnvelope {
    let buyer = sign::secret_from_label(BUYER_SPEND_KEY);
    let book = catalog
        .listings
        .iter()
        .find(|listing| listing.title == "Paperback novel")
        .expect("fixture shop has a book");
    OrderEnvelope::sign(
        OrderPlaintext {
            buyer: sign::xonly(&buyer),
            listing_hash: book.content_hash().expect("book body encodes"),
            quantity: 1,
            note: "Please pack it flat.".to_string(),
            ship_to: "12 Cedar Street".to_string(),
        },
        &buyer,
    )
    .expect("fixture order signs")
}

pub fn fixture_dispute(catalog: &Catalog) -> DisputeEnvelope {
    let buyer_secret = sign::secret_from_label(BUYER_SPEND_KEY);
    let vendor_spend = sign::xonly(&sign::secret_from_label(VENDOR_SPEND_KEY));
    let buyer = sign::xonly(&buyer_secret);
    let OrderEnvelope::Plaintext { order, .. } = fixture_order(catalog) else {
        panic!("fixture order is plaintext");
    };
    DisputeEnvelope::sign(
        DisputePlaintext {
            opener: buyer,
            order,
            claim: "The cover arrived torn.".to_string(),
            escrow: EscrowTerms {
                buyer,
                vendor: vendor_spend,
                moderator: compressed(MODERATOR_KEY),
                moderator_fee: 1_000,
                dispute_deadline: 720,
                vendor_claim_height: 864,
                refund_locktime: 1008,
                buyer_wins: EscrowBranch {
                    adaptor_point: compressed(ADAPTOR_POINT),
                    letter: Vec::new(),
                },
                vendor_wins: EscrowBranch {
                    adaptor_point: compressed(VENDOR_WINS_POINT),
                    letter: Vec::new(),
                },
            },
        },
        &buyer_secret,
    )
    .expect("fixture dispute signs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_catalog_verifies_and_keeps_the_order_private() {
        let catalog = fixture_catalog();
        catalog.verify().unwrap();
        assert_eq!(catalog.listings.len(), LISTINGS.len());
        assert!(catalog.listings.len() > 4);
        let mut categories = std::collections::BTreeSet::new();
        for listing in &catalog.listings {
            assert!(!listing.description.trim().is_empty());
            categories.insert(listing.category.as_str());
        }
        assert!(categories.len() >= 6);
        let mut sellers = std::collections::BTreeSet::new();
        for listing in &catalog.listings {
            assert!(!listing.seller.trim().is_empty());
            assert!(listing.stock > 0);
            assert!(listing.listed_on >= 20260101);
            sellers.insert(listing.seller.as_str());
        }
        assert!(sellers.len() >= 3);
        assert!(catalog.listings.len() >= 30);
        assert!(catalog.stores.iter().all(|store| store.rating == Some(ContentRating::General)));
        assert!(catalog
            .listings
            .iter()
            .all(|listing| listing.content == Some(ContentLabel::GENERAL)));
        assert_eq!(catalog.moderator.name, "June Pell");
        assert!(catalog.stores.iter().all(|store| catalog.moderator.id != *store.id.as_bytes()));

        let order = fixture_order(&catalog);
        let order_bytes = order.encode().unwrap();
        assert_eq!(OrderEnvelope::decode(&order_bytes).unwrap(), order);
        order.verify().unwrap();

        let dispute = fixture_dispute(&catalog);
        let dispute_bytes = dispute.encode().unwrap();
        let decoded = DisputeEnvelope::decode(&dispute_bytes).unwrap();
        assert_eq!(decoded, dispute);
        dispute.verify().unwrap();

        let catalog_bytes = catalog.encode().unwrap();
        let ship_to = b"12 Cedar Street";
        assert!(
            !catalog_bytes
                .windows(ship_to.len())
                .any(|window| window == ship_to),
            "the public catalog must not carry the order"
        );

        let DisputeEnvelope::Plaintext {
            mut dispute,
            signature,
        } = decoded
        else {
            panic!("plaintext dispute");
        };
        assert!(dispute.escrow.buyer_wins.letter.is_empty());
        assert_ne!(
            dispute.escrow.buyer_wins.adaptor_point,
            dispute.escrow.vendor_wins.adaptor_point
        );
        for store in &catalog.stores {
            assert_ne!(dispute.escrow.buyer, *store.id.as_bytes());
            assert_ne!(dispute.escrow.vendor, *store.id.as_bytes());
        }
        assert_ne!(dispute.escrow.buyer, dispute.escrow.vendor);
        dispute.claim.push('!');
        let tampered = DisputeEnvelope::Plaintext { dispute, signature };
        assert_eq!(tampered.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn each_demo_seller_is_its_own_store_with_its_own_keys() {
        let catalog = fixture_catalog();
        assert_eq!(catalog.stores.len(), 3);
        let mut seen = Vec::new();
        for store in &catalog.stores {
            let keys = fixture_store_keys(&store.name).unwrap();
            assert_eq!(sign::xonly(&keys.store), *store.id.as_bytes());
            assert_ne!(keys.mailbox, keys.store.secret_bytes(), "the mailbox key is not the store scalar");
            let details = store.details.as_ref().unwrap();
            assert_eq!(store.version(), 3);
            assert_eq!(details.moderators, vec![catalog.moderator.id]);
            assert!(!details.profile.tagline.is_empty() && !details.profile.ships_to.is_empty());
            let media = fixture_media();
            for hash in [details.profile.avatar.unwrap(), details.profile.banner.unwrap()] {
                assert!(media.iter().any(|bytes| sign::content_hash(bytes) == hash));
            }
            seen.extend([*store.id.as_bytes(), details.mailbox_key]);
            let listings: Vec<_> = catalog.listings.iter().filter(|l| l.store_id == store.id).collect();
            assert!(!listings.is_empty());
            assert!(listings.iter().all(|listing| listing.seller == store.name));
        }
        for (i, key) in seen.iter().enumerate() {
            assert!(!seen[i + 1..].contains(key), "store and mailbox keys never repeat");
        }
        assert!(fixture_store_keys("Someone Else").is_none());
    }

    #[test]
    fn a_listing_moved_to_another_store_does_not_verify() {
        let mut catalog = fixture_catalog();
        let other = catalog.stores[1].id;
        let listing = catalog.listings.iter_mut().find(|l| l.store_id != other).unwrap();
        listing.store_id = other;
        assert_eq!(catalog.verify(), Err(Error::BadSignature));

        let mut catalog = fixture_catalog();
        catalog.stores.remove(2);
        assert_eq!(catalog.verify(), Err(Error::StoreMismatch));
        let mut catalog = fixture_catalog();
        catalog.stores.push(catalog.stores[0].clone());
        assert_eq!(catalog.verify(), Err(Error::StoreMismatch));
    }

    #[test]
    fn signed_catalog_round_trips_and_rejects_a_bad_signature() {
        let catalog = fixture_catalog();
        let bytes = catalog.encode().unwrap();
        let decoded = Catalog::decode_signed(&bytes, catalog.moderator.clone()).unwrap();
        assert_eq!(decoded, catalog);
        decoded.verify().unwrap();

        let mut bad = bytes.clone();
        let last = bad.last_mut().unwrap();
        *last ^= 0xff;
        let decoded = Catalog::decode_signed(&bad, catalog.moderator.clone()).unwrap();
        assert_eq!(decoded.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn catalog_mixes_unlabeled_and_labeled_listings() {
        let mut catalog = fixture_catalog();
        let pen = catalog
            .listings
            .iter_mut()
            .find(|listing| listing.title == "Dip pen")
            .unwrap();
        // The same listing, signed by the version 4 code before labels existed.
        let mut legacy = pen.clone();
        legacy.content = None;
        let encoded = legacy.encode().unwrap();
        let body = &encoded[..encoded.len() - sign::SIG_LEN];
        legacy.signature = sign::sign("ob/listing/v1", body, &fixture_store_keys("Hale Paper").unwrap().store);
        *pen = legacy;
        catalog.verify().unwrap();
        let decoded =
            Catalog::decode_signed(&catalog.encode().unwrap(), catalog.moderator.clone()).unwrap();
        assert_eq!(decoded, catalog);
        decoded.verify().unwrap();
        assert_eq!(
            decoded
                .listings
                .iter()
                .filter(|l| l.content.is_none())
                .count(),
            1
        );
    }
}
