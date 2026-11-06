//! Local shop node: signed fixture catalog, sealed orders, and checkout rails.
//!
//! The HTTP server is loopback-only. It does not dial Tor and it does not
//! broadcast Bitcoin transactions.

#![forbid(unsafe_code)]

mod addresses;
mod buyer_orders;
mod http;
mod orders;
mod rates;
mod vault;

#[cfg(test)]
mod checkout_tests;

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{Amount, Network};
use openbazaar_pay::{Bolt12Offer, SilentPaymentCode};
use openbazaar_protocol::{
    fixture_catalog, fixture_keys, Catalog, DirectPay, Error as ProtocolError, Listing, Store,
};
use serde_json::{json, Value};
use tormail::Mailbox;

use addresses::AddressBook;
use buyer_orders::BuyerOrders;
use rates::Rates;
use vault::Vault;

pub use http::http_response;
pub use orders::{OrderError, ESCROW_TIMEOUT_HEIGHT};

pub fn default_mailbox_dir() -> PathBuf {
    PathBuf::from("data/mailbox")
}

#[derive(Debug)]
pub enum Error {
    Protocol(ProtocolError),
    Detail(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Protocol(err) => write!(f, "{err}"),
            Error::Detail(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) struct Keys {
    store: bitcoin::secp256k1::SecretKey,
    spend: bitcoin::secp256k1::SecretKey,
    scan: bitcoin::secp256k1::SecretKey,
    buyer: bitcoin::secp256k1::SecretKey,
    moderator: bitcoin::secp256k1::SecretKey,
}

pub struct Node {
    pub(crate) catalog: Catalog,
    pub(crate) mailbox: Mailbox,
    pub(crate) keys: Keys,
    pairing_token: String,
    /// True only in the process that created `pairing.token`.
    pairing_fresh: bool,
    rates: Rates,
    pub(crate) addresses: Mutex<AddressBook>,
    pub(crate) buyer_orders: Mutex<BuyerOrders>,
}

impl Node {
    pub fn open(dir: impl Into<PathBuf>, catalog: Catalog) -> Result<Self, Error> {
        let fixture = fixture_keys();
        let mailbox = Mailbox::open(dir).map_err(|err| Error::Detail(err.to_string()))?;
        let (pairing_token, pairing_fresh) = load_pairing(mailbox.path())?;
        let vault = Vault::open(mailbox.path())?;
        let addresses = AddressBook::open(mailbox.path(), vault.clone())?;
        let buyer_orders = BuyerOrders::open(mailbox.path(), vault)?;
        Ok(Self {
            catalog,
            mailbox,
            keys: Keys {
                store: fixture.store,
                spend: fixture.spend,
                scan: fixture.scan,
                buyer: fixture.buyer,
                moderator: fixture.moderator,
            },
            pairing_token,
            pairing_fresh,
            rates: Rates::new(),
            addresses: Mutex::new(addresses),
            buyer_orders: Mutex::new(buyer_orders),
        })
    }

    pub(crate) fn rates_value(&self) -> Value {
        self.rates.value()
    }

    pub fn mailbox_path(&self) -> &Path {
        self.mailbox.path()
    }

    pub(crate) fn pairing_token(&self) -> &str {
        &self.pairing_token
    }

    /// The token, only when this process just wrote it.
    ///
    /// `main` prints that value once. A later boot loads the same file and
    /// returns `None`, so the secret is not written to the log again.
    pub fn fresh_pairing_token(&self) -> Option<&str> {
        self.pairing_fresh.then_some(self.pairing_token.as_str())
    }

    pub fn serve(&self, port: u16) -> std::io::Result<()> {
        http::serve(self, port)
    }
}

pub fn load_catalog() -> Result<Catalog, Error> {
    let mut catalog = fixture_catalog();
    install_direct_pay(&mut catalog)?;
    catalog.verify().map_err(Error::Protocol)?;
    Ok(catalog)
}

/// Replace placeholder payment strings with a real BOLT 12 offer and BIP 352 code.
fn install_direct_pay(catalog: &mut Catalog) -> Result<(), Error> {
    let keys = fixture_keys();
    let mut offer = catalog.store.direct_pay.bolt12_offer.clone();
    let mut code = catalog.store.direct_pay.silent_payment_code.clone();
    let offer_real = Bolt12Offer::parse(&offer).is_ok();
    let code_real = SilentPaymentCode::parse(&code).is_ok();
    if offer_real && code_real {
        return Ok(());
    }
    if !code_real {
        code = SilentPaymentCode::from_keys(&keys.scan, &keys.spend, Network::Bitcoin)
            .map_err(|err| Error::Detail(err.to_string()))?
            .to_string();
    }
    if !offer_real {
        let secp = Secp256k1::new();
        let pubkey = PublicKey::from_secret_key(&secp, &keys.store);
        offer = Bolt12Offer::create(
            pubkey,
            Amount::from_sat(18_000),
            "Marlow Goods",
            Network::Bitcoin,
        )
        .map_err(|err| Error::Detail(err.to_string()))?
        .to_string();
    }
    catalog.store = Store::sign(
        catalog.store.name.clone(),
        catalog.store.region.clone(),
        DirectPay {
            bolt12_offer: offer,
            silent_payment_code: code,
        },
        &keys.store,
    )
    .map_err(Error::Protocol)?;
    Ok(())
}

pub(crate) fn accepts_moderator(catalog: &Catalog, listing: &Listing) -> bool {
    listing.moderated
        && listing.store_id == catalog.store.id
        && !catalog.moderator.name.is_empty()
}

pub(crate) fn find_listing<'a>(catalog: &'a Catalog, hash: &str) -> Option<&'a Listing> {
    let hash = hash.trim().trim_end_matches('/');
    catalog.listings.iter().find(|listing| {
        listing
            .content_hash()
            .ok()
            .is_some_and(|value| value.to_hex() == hash)
    })
}

pub(crate) fn listing_value(catalog: &Catalog, listing: &Listing) -> Result<Value, ProtocolError> {
    Ok(json!({
        "hash": listing.content_hash()?.to_hex(),
        "storeId": listing.store_id.to_hex(),
        "title": listing.title,
        "category": listing.category,
        "priceSats": listing.price_sats,
        "shipFrom": listing.ship_from,
        "kind": listing.kind.as_str(),
        "description": listing.description,
        "seller": listing.seller,
        "stock": listing.stock,
        "listedOn": listing.listed_on,
        "product": listing.product,
        "shippingSats": listing.shipping_sats,
        "freeOverSats": listing.free_over_sats,
        "acceptsModerator": accepts_moderator(catalog, listing),
    }))
}

pub fn catalog_value(catalog: &Catalog) -> Result<Value, ProtocolError> {
    let mut listings = Vec::with_capacity(catalog.listings.len());
    for listing in &catalog.listings {
        listings.push(listing_value(catalog, listing)?);
    }
    Ok(json!({
        "store": {
            "id": catalog.store.id.to_hex(),
            "name": catalog.store.name,
            "region": catalog.store.region,
        },
        "moderator": {
            "name": catalog.moderator.name,
            "id": catalog.moderator.id_hex(),
        },
        "listings": listings,
    }))
}

/// Shared secret for `POST /orders`, stored as `pairing.token` in the node data dir.
///
/// The first boot creates it. [`Node::fresh_pairing_token`] is how the process
/// learns that it should print the token once. `GET /pair` on the loopback
/// listener is how the local shop reads it after that.
fn load_pairing(dir: &Path) -> Result<(String, bool), Error> {
    let path = dir.join("pairing.token");
    if let Some(token) = read_pairing(&path)? {
        return Ok((token, false));
    }
    if path.exists() {
        fs::remove_file(&path).map_err(|err| Error::Detail(err.to_string()))?;
    }
    let token = generate_pairing_token();
    if write_pairing(&path, &token)? {
        return Ok((token, true));
    }
    // Another process created the file between the read and the write.
    match read_pairing(&path)? {
        Some(token) => Ok((token, false)),
        None => Err(Error::Detail(
            "the pairing token could not be read".to_string(),
        )),
    }
}

fn read_pairing(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let token = text.trim();
            if is_pairing_token(token) {
                Ok(Some(token.to_string()))
            } else {
                Ok(None)
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Detail(err.to_string())),
    }
}

fn is_pairing_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn generate_pairing_token() -> String {
    let mut bytes = [0u8; 32];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns `true` when this process created the file.
fn write_pairing(path: &Path, token: &str) -> Result<bool, Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(err) => return Err(Error::Detail(err.to_string())),
    };
    file.write_all(token.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|err| Error::Detail(err.to_string()))?;
    Ok(true)
}

pub fn catalog_json(catalog: &Catalog) -> Result<String, ProtocolError> {
    let value = catalog_value(catalog)?;
    Ok(serde_json::to_string_pretty(&value).expect("catalog json is finite"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn fixture_catalog_verifies() {
        let catalog = load_catalog().unwrap();
        assert_eq!(catalog.store.name, "Marlow Goods");
        assert_eq!(catalog.moderator.name, "June Pell");
        assert!(catalog
            .listings
            .iter()
            .any(|listing| listing.kind.as_str() == "physical"));
        assert!(catalog
            .listings
            .iter()
            .any(|listing| listing.kind.as_str() == "digital"));
        assert!(catalog.store.direct_pay.bolt12_offer.starts_with("lno1"));
        assert!(catalog
            .store
            .direct_pay
            .silent_payment_code
            .starts_with("sp1"));
        assert!(Bolt12Offer::parse(&catalog.store.direct_pay.bolt12_offer).is_ok());
        assert!(SilentPaymentCode::parse(&catalog.store.direct_pay.silent_payment_code).is_ok());
    }

    #[test]
    fn shared_ui_catalog_matches_signed_records() {
        let catalog = load_catalog().unwrap();
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/mobile/src/catalog.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let committed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(committed, catalog_value(&catalog).unwrap());
    }

    #[test]
    fn catalog_response_carries_the_json_body() {
        let json = catalog_json(&load_catalog().unwrap()).unwrap();
        let response = http_response("200 OK", "application/json", json.as_bytes());
        let text = String::from_utf8(response).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(header.contains("200 OK"));
        assert!(header.contains(&format!("Content-Length: {}", json.len())));
        assert!(header.contains("Access-Control-Allow-Origin: http://127.0.0.1:4173"));
        assert_eq!(body, json);
        assert!(body.contains("Marlow Goods"));
        assert!(body.contains("Paperback novel"));
        assert!(body.contains("June Pell"));
    }
}
