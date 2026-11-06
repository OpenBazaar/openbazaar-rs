//! Checkout: verify the listing, seal a tormail letter, and describe how to pay.
//!
//! Each seller order gets its own payment terms from [`crate::instructions`]:
//! a fresh address for direct pay, or, for moderated pay, a fresh buyer
//! escrow key and a request for terms sent to the vendor's node. Nothing
//! here broadcasts a transaction.

use std::collections::BTreeMap;
use std::fmt;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use openbazaar_protocol::{vendor_mail_key, Catalog, Listing, ListingKind};
use serde_json::{json, Value};
use tormail::{
    seal_order, ListingHash, Order, PaymentRail, ReplyKeypair, SealedLetter, StoreKey, StoreSecret,
};

use crate::buyer_orders::{now_secs, BuyerOrder};
use crate::{accepts_moderator, find_listing, Node};

#[derive(Debug)]
pub enum OrderError {
    UnknownListing,
    Invalid(&'static str),
    Failed(String),
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderError::UnknownListing => f.write_str("That listing is not in this shop."),
            OrderError::Invalid(message) => f.write_str(message),
            OrderError::Failed(detail) => write!(f, "The order could not be sealed. {detail}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Settlement {
    Direct,
    Escrow,
}

impl Settlement {
    fn as_str(self) -> &'static str {
        match self {
            Settlement::Direct => "direct",
            Settlement::Escrow => "escrow",
        }
    }

    fn initial_status(self) -> &'static str {
        match self {
            Settlement::Direct => crate::watch::AWAITING_PAYMENT,
            Settlement::Escrow => crate::watch::AWAITING_TERMS,
        }
    }
}

struct ShipTo {
    name: String,
    street: String,
    street2: String,
    city: String,
    region: String,
    postal_code: String,
    country: String,
}

impl Node {
    pub fn place_order(&self, body: &Value) -> Result<Value, OrderError> {
        // The vendor is the store that signed the listing this node already
        // holds. The client does not name a peer, seed, onion, or vendor URL.
        if client_named_destination(body) {
            return Err(OrderError::Invalid(
                "The node delivers this order. A destination was not accepted.",
            ));
        }
        expect_order_fields(body)?;
        let catalog = self.catalog();
        let lines = order_lines(&catalog, body)?;
        let seller = lines[0].0.seller.clone();
        if lines.iter().any(|(listing, _)| listing.seller != seller) {
            return Err(OrderError::Invalid(
                "Checkout one seller at a time.",
            ));
        }
        let settlement = settlement_of(body.get("settlement"))?;
        self.allow_settlement(settlement, &seller)?;
        if settlement == Settlement::Escrow
            && lines
                .iter()
                .any(|(listing, _)| !accepts_moderator(&catalog, listing))
        {
            return Err(OrderError::Invalid(
                "This listing does not accept a moderator.",
            ));
        }
        let needs_address = lines.iter().any(|(listing, _)| listing.kind == ListingKind::Physical);
        let needs_note = lines.iter().any(|(listing, _)| listing.kind == ListingKind::Digital);
        let (address_label, ship_text) = if needs_address {
            self.ship_text(body)?
        } else {
            (String::new(), String::new())
        };
        let note = if needs_note {
            delivery_note(body.get("deliveryNote"))?
        } else {
            String::new()
        };
        let (items, shipping) = package_shipping(&lines)?;
        let total = items
            .checked_add(shipping)
            .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
        let sealed_lines = line_values(&lines)?;
        let (listing, quantity) = &lines[0];
        let letter_id = self.seal_package(&lines, settlement, &ship_text, &note, shipping)?;
        let checkout_id = random_id();
        let mut response = json!({
            "orderId": letter_id,
            "letterId": letter_id,
            "checkoutId": checkout_id,
            "listingHash": sealed_lines[0]["listingHash"],
            "title": listing.title,
            "quantity": quantity,
            "priceSats": listing.price_sats,
            "itemsSats": items,
            "shippingSats": shipping,
            "totalSats": total,
            "settlement": settlement.as_str(),
            "sealed": true,
            "storeName": catalog.store.name,
            "seller": seller,
            "storeId": listing.store_id.to_hex(),
            "addressLabel": address_label,
            "lines": sealed_lines,
            "status": settlement.initial_status(),
            "network": self.network.as_str(),
        });
        let (payment, escrow) = self.payment_for(settlement, total, &seller)?;
        if let Some(payment) = &payment {
            response["payment"] = payment.clone();
        }
        if let Some(escrow) = &escrow {
            response["escrow"] = escrow.clone();
        }
        self.remember(
            checkout_id,
            letter_id.clone(),
            now_secs(),
            seller,
            listing.store_id.to_hex(),
            response["lines"].as_array().cloned().unwrap_or_default(),
            items,
            shipping,
            total,
            settlement,
            payment,
            escrow,
            address_label,
        )?;
        if settlement == Settlement::Escrow {
            let mut escrow = response.get("escrow").cloned();
            self.request_terms(&letter_id, &mut escrow);
            if let Some(escrow) = escrow {
                response["escrow"] = escrow;
            }
        }
        Ok(response)
    }

    /// One checkout, one sealed letter and one buyer record per seller.
    ///
    /// Every package is checked before any letter is sealed. A bad listing
    /// in one package leaves the mailbox untouched.
    pub fn checkout(&self, body: &Value) -> Result<Value, OrderError> {
        if client_named_destination(body) {
            return Err(OrderError::Invalid(
                "The node delivers this order. A destination was not accepted.",
            ));
        }
        expect_fields(body, CHECKOUT_FIELDS)?;
        let catalog = self.catalog();
        let packages = checkout_packages(&catalog, body)?;
        for package in &packages {
            self.allow_settlement(package.settlement, &package.lines[0].0.seller)?;
        }
        let needs_address = packages.iter().any(|package| {
            package.lines.iter().any(|(listing, _)| listing.kind == ListingKind::Physical)
        });
        let needs_note = packages.iter().any(|package| {
            package.lines.iter().any(|(listing, _)| listing.kind == ListingKind::Digital)
        });
        let (address_label, ship_text) = if needs_address {
            self.ship_text(body)?
        } else {
            (String::new(), String::new())
        };
        let note = if needs_note {
            delivery_note(body.get("deliveryNote"))?
        } else {
            String::new()
        };
        let mut sealed = Vec::with_capacity(packages.len());
        for package in &packages {
            let seller = package.lines[0].0.seller.clone();
            let vendor = vendor_key(&seller)?;
            let letter = seal_letter(
                &package.lines,
                package.settlement,
                letter_ship(&package.lines, &ship_text, &note),
                package.shipping,
                &note,
                &vendor,
            )?;
            sealed.push(letter);
        }
        let checkout_id = random_id();
        let created = now_secs();
        let mut orders = Vec::with_capacity(packages.len());
        let mut items_total = 0u64;
        let mut shipping_total = 0u64;
        for (package, letter) in packages.iter().zip(sealed) {
            let seller = package.lines[0].0.seller.clone();
            let listing = package.lines[0].0;
            let id = self
                .mailbox
                .accept(&letter)
                .map_err(|err| OrderError::Failed(err.to_string()))?
                .as_str();
            let total = package
                .items
                .checked_add(package.shipping)
                .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
            items_total = items_total
                .checked_add(package.items)
                .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
            shipping_total = shipping_total
                .checked_add(package.shipping)
                .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
            let lines = line_values(&package.lines)?;
            let (payment, mut escrow) = self.payment_for(package.settlement, total, &seller)?;
            self.remember(
                checkout_id.clone(),
                id.clone(),
                created,
                seller.clone(),
                listing.store_id.to_hex(),
                lines.clone(),
                package.items,
                package.shipping,
                total,
                package.settlement,
                payment.clone(),
                escrow.clone(),
                address_label.clone(),
            )?;
            if package.settlement == Settlement::Escrow {
                self.request_terms(&id, &mut escrow);
            }
            let mut order = json!({
                "orderId": id,
                "letterId": id,
                "seller": seller,
                "storeId": listing.store_id.to_hex(),
                "shipFrom": listing.ship_from,
                "lines": lines,
                "itemsSats": package.items,
                "shippingSats": package.shipping,
                "totalSats": total,
                "settlement": package.settlement.as_str(),
                "addressLabel": address_label,
                "status": package.settlement.initial_status(),
                "network": self.network.as_str(),
            });
            if let Some(payment) = payment {
                order["payment"] = payment;
            }
            if let Some(escrow) = escrow {
                order["escrow"] = escrow;
            }
            orders.push(order);
        }
        Ok(json!({
            "checkoutId": checkout_id,
            "sealed": true,
            "itemsSats": items_total,
            "shippingSats": shipping_total,
            "totalSats": items_total + shipping_total,
            "created": created,
            "orders": orders,
        }))
    }

    fn seal_package(
        &self,
        lines: &[(&Listing, u32)],
        settlement: Settlement,
        ship_text: &str,
        note: &str,
        shipping: u64,
    ) -> Result<String, OrderError> {
        let seller = &lines[0].0.seller;
        let vendor = vendor_key(seller)?;
        let letter = seal_letter(lines, settlement, letter_ship(lines, ship_text, note), shipping, note, &vendor)?;
        let id = self
            .mailbox
            .accept(&letter)
            .map_err(|err| OrderError::Failed(err.to_string()))?;
        Ok(id.as_str())
    }

    /// Escrow is a testnet4 demo with a vendor on another node. Refuse it
    /// before any letter is sealed.
    fn allow_settlement(&self, settlement: Settlement, seller: &str) -> Result<(), OrderError> {
        if settlement == Settlement::Escrow {
            self.escrow_ready(seller)?;
        }
        Ok(())
    }

    /// Ask the vendor's node for terms once the order is stored. A failure is
    /// kept on the order (and returned in `escrow.error`); checkout still succeeds.
    fn request_terms(&self, id: &str, escrow: &mut Option<Value>) {
        if let (Err(err), Some(value)) = (self.send_escrow_request(id), escrow.as_mut()) {
            value["error"] = json!(err);
        }
    }

    fn payment_for(
        &self,
        settlement: Settlement,
        total: u64,
        seller: &str,
    ) -> Result<(Option<Value>, Option<Value>), OrderError> {
        match settlement {
            Settlement::Direct => Ok((Some(self.direct_payment(total, seller)?), None)),
            Settlement::Escrow => Ok((None, Some(self.escrow_request(total, seller)?))),
        }
    }

    fn remember(
        &self,
        checkout_id: String,
        id: String,
        created: u64,
        seller: String,
        store_id: String,
        lines: Vec<Value>,
        items: u64,
        shipping: u64,
        total: u64,
        settlement: Settlement,
        payment: Option<Value>,
        escrow: Option<Value>,
        address_label: String,
    ) -> Result<(), OrderError> {
        self.buyer_orders
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .record(BuyerOrder {
                id,
                checkout_id,
                created,
                seller,
                store_id,
                lines,
                items_sats: items,
                shipping_sats: shipping,
                total_sats: total,
                settlement: settlement.as_str().to_string(),
                payment,
                escrow,
                address_label,
                status: settlement.initial_status().to_string(),
                network: self.network.as_str().to_string(),
                received_sats: 0,
                confirmations: 0,
                txids: Vec::new(),
                pay_txid: None,
                paying: false,
                pay_error: None,
            })
            .map_err(OrderError::Failed)
    }

    fn ship_text(&self, body: &Value) -> Result<(String, String), OrderError> {
        let has_id = body.get("addressId").is_some();
        let has_ship = body.get("shipTo").is_some();
        if has_id == has_ship {
            return Err(OrderError::Invalid(
                "Choose a saved address or enter a new one.",
            ));
        }
        if let Some(id) = body.get("addressId").and_then(Value::as_str) {
            let book = self.addresses.lock().unwrap_or_else(|err| err.into_inner());
            let Some(address) = book.get(id) else {
                return Err(OrderError::Invalid("That address is not on this node."));
            };
            return Ok((address.label.clone(), address.letter_text()));
        }
        let ship = ship_to_of(body.get("shipTo"))?;
        let label = body
            .get("addressLabel")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        Ok((label, format_ship_to(ship)))
    }
}


struct PackagePlan<'a> {
    lines: Vec<(&'a Listing, u32)>,
    settlement: Settlement,
    items: u64,
    shipping: u64,
}

const CHECKOUT_FIELDS: &[&str] = &[
    "addressId",
    "shipTo",
    "addressLabel",
    "deliveryNote",
    "packages",
];

const PACKAGE_FIELDS: &[&str] = &["settlement", "lines"];

fn checkout_packages<'a>(catalog: &'a Catalog, body: &Value) -> Result<Vec<PackagePlan<'a>>, OrderError> {
    let Some(list) = body.get("packages").and_then(Value::as_array) else {
        return Err(OrderError::Invalid("Choose a package."));
    };
    if list.is_empty() {
        return Err(OrderError::Invalid("Choose a package."));
    }
    let mut plans = Vec::new();
    let mut sellers = Vec::new();
    for package in list {
        let Some(map) = package.as_object() else {
            return Err(OrderError::Invalid("The order could not be read."));
        };
        if map.keys().any(|key| !PACKAGE_FIELDS.contains(&key.as_str())) {
            return Err(OrderError::Invalid("That field is not part of an order."));
        }
        let lines = order_lines(catalog, package)?;
        let seller = lines[0].0.seller.clone();
        if lines.iter().any(|(listing, _)| listing.seller != seller) {
            return Err(OrderError::Invalid("Each package is one seller."));
        }
        if sellers.iter().any(|seen| seen == &seller) {
            return Err(OrderError::Invalid("Each seller is one package."));
        }
        sellers.push(seller);
        let settlement = settlement_of(package.get("settlement"))?;
        if settlement == Settlement::Escrow
            && lines
                .iter()
                .any(|(listing, _)| !accepts_moderator(catalog, listing))
        {
            return Err(OrderError::Invalid(
                "This listing does not accept a moderator.",
            ));
        }
        let (items, shipping) = package_shipping(&lines)?;
        plans.push(PackagePlan {
            lines,
            settlement,
            items,
            shipping,
        });
    }
    Ok(plans)
}

fn package_shipping(lines: &[(&Listing, u32)]) -> Result<(u64, u64), OrderError> {
    let mut items = 0u64;
    for (listing, quantity) in lines {
        let line_total = listing
            .price_sats
            .checked_mul(u64::from(*quantity))
            .filter(|value| *value > 0)
            .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
        items = items
            .checked_add(line_total)
            .ok_or(OrderError::Invalid("Enter a quantity of at most 99."))?;
    }
    let mut rate = None;
    for (listing, _) in lines {
        if listing.kind != ListingKind::Physical {
            continue;
        }
        match rate {
            None => rate = Some((listing.shipping_sats, listing.free_over_sats)),
            Some((shipping, free))
                if shipping == listing.shipping_sats && free == listing.free_over_sats => {}
            Some(_) => {
                return Err(OrderError::Invalid(
                    "This seller's shipping does not match across the package.",
                ));
            }
        }
    }
    let shipping = match rate {
        None => 0,
        Some((_, free)) if free > 0 && items >= free => 0,
        Some((shipping, _)) => shipping,
    };
    Ok((items, shipping))
}

fn line_values(lines: &[(&Listing, u32)]) -> Result<Vec<Value>, OrderError> {
    let mut out = Vec::new();
    for (listing, quantity) in lines {
        let hash = listing
            .content_hash()
            .map_err(|err| OrderError::Failed(err.to_string()))?;
        out.push(json!({
            "listingHash": hash.to_hex(),
            "title": listing.title,
            "quantity": quantity,
            "priceSats": listing.price_sats,
        }));
    }
    Ok(out)
}

fn letter_ship(lines: &[(&Listing, u32)], ship_text: &str, note: &str) -> String {
    let physical = lines.iter().any(|(listing, _)| listing.kind == ListingKind::Physical);
    if physical {
        ship_text.to_string()
    } else {
        note.to_string()
    }
}

fn seal_letter(
    lines: &[(&Listing, u32)],
    settlement: Settlement,
    ship_to: String,
    shipping: u64,
    note: &str,
    vendor: &StoreKey,
) -> Result<SealedLetter, OrderError> {
    let mut options = BTreeMap::new();
    options.insert("settlement".to_string(), settlement.as_str().to_string());
    options.insert("shipping".to_string(), shipping.to_string());
    let physical = lines.iter().any(|(listing, _)| listing.kind == ListingKind::Physical);
    let digital = lines.iter().any(|(listing, _)| listing.kind == ListingKind::Digital);
    if physical && digital && !note.is_empty() {
        options.insert("delivery".to_string(), note.to_string());
    }
    let (first, first_qty) = lines[0];
    let first_hash = first
        .content_hash()
        .map_err(|err| OrderError::Failed(err.to_string()))?;
    for (listing, quantity) in lines.iter().skip(1) {
        let hash = listing
            .content_hash()
            .map_err(|err| OrderError::Failed(err.to_string()))?
            .to_hex();
        options.insert(hash, quantity.to_string());
    }
    let reply = ReplyKeypair::generate();
    let rail = match settlement {
        Settlement::Direct => PaymentRail::Lightning,
        Settlement::Escrow => PaymentRail::OnChain,
    };
    let order = Order::new(
        ListingHash(*first_hash.as_bytes()),
        first_qty,
        options,
        ship_to,
        rail,
        *reply.public_key(),
    )
    .map_err(|err| OrderError::Failed(err.to_string()))?;
    seal_order(&order, vendor).map_err(|err| OrderError::Failed(err.to_string()))
}

fn vendor_key(seller: &str) -> Result<StoreKey, OrderError> {
    let secret = vendor_mail_key(seller)
        .ok_or(OrderError::Invalid("That seller has no mailbox key."))?;
    let secret = StoreSecret::from_bytes(secret.secret_bytes())
        .map_err(|err| OrderError::Failed(err.to_string()))?;
    Ok(secret.public_key())
}

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const ORDER_FIELDS: &[&str] = &[
    "listingHash",
    "quantity",
    "settlement",
    "shipTo",
    "deliveryNote",
    "addressId",
    "addressLabel",
    "lines",
];

fn expect_order_fields(body: &Value) -> Result<(), OrderError> {
    expect_fields(body, ORDER_FIELDS)
}

fn expect_fields(body: &Value, allowed: &[&str]) -> Result<(), OrderError> {
    let Some(map) = body.as_object() else {
        return Err(OrderError::Invalid("The order could not be read."));
    };
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(OrderError::Invalid("That field is not part of an order."));
    }
    Ok(())
}

fn order_lines<'a>(
    catalog: &'a Catalog,
    body: &Value,
) -> Result<Vec<(&'a Listing, u32)>, OrderError> {
    if let Some(lines) = body.get("lines") {
        if body.get("listingHash").is_some() || body.get("quantity").is_some() {
            return Err(OrderError::Invalid("That field is not part of an order."));
        }
        let Some(list) = lines.as_array() else {
            return Err(OrderError::Invalid("The order could not be read."));
        };
        if list.is_empty() {
            return Err(OrderError::Invalid("Choose a listing."));
        }
        let mut out = Vec::new();
        for line in list {
            let Some(map) = line.as_object() else {
                return Err(OrderError::Invalid("The order could not be read."));
            };
            if map.keys().any(|key| key != "listingHash" && key != "quantity") {
                return Err(OrderError::Invalid("That field is not part of an order."));
            }
            let hash = map.get("listingHash").and_then(Value::as_str).unwrap_or("");
            let listing = find_listing(catalog, hash).ok_or(OrderError::UnknownListing)?;
            out.push((listing, quantity_of(map.get("quantity"))?));
        }
        return Ok(out);
    }
    let hash = body.get("listingHash").and_then(Value::as_str).unwrap_or("");
    let listing = find_listing(catalog, hash).ok_or(OrderError::UnknownListing)?;
    Ok(vec![(listing, quantity_of(body.get("quantity"))?)])
}

fn quantity_of(value: Option<&Value>) -> Result<u32, OrderError> {
    let Some(quantity) = value.and_then(Value::as_u64) else {
        return Err(OrderError::Invalid("Enter a quantity of at least 1."));
    };
    if quantity == 0 {
        return Err(OrderError::Invalid("Enter a quantity of at least 1."));
    }
    if quantity > 99 {
        return Err(OrderError::Invalid("Enter a quantity of at most 99."));
    }
    Ok(quantity as u32)
}

fn settlement_of(value: Option<&Value>) -> Result<Settlement, OrderError> {
    match value.and_then(Value::as_str) {
        Some("direct") => Ok(Settlement::Direct),
        Some("escrow") => Ok(Settlement::Escrow),
        _ => Err(OrderError::Invalid(
            "Choose direct pay or moderated escrow.",
        )),
    }
}

fn ship_to_of(value: Option<&Value>) -> Result<ShipTo, OrderError> {
    let Some(record) = value.and_then(Value::as_object) else {
        return Err(OrderError::Invalid(
            "Enter the ship-to name, street, city, region, and postal code.",
        ));
    };
    const SHIP_FIELDS: &[&str] = &[
        "name",
        "street",
        "street2",
        "city",
        "region",
        "postalCode",
        "country",
    ];
    if record.keys().any(|key| !SHIP_FIELDS.contains(&key.as_str())) {
        return Err(OrderError::Invalid("That field is not part of an order."));
    }
    Ok(ShipTo {
        name: address_field(record.get("name"))?,
        street: address_field(record.get("street"))?,
        street2: optional_address_field(record.get("street2"))?,
        city: address_field(record.get("city"))?,
        region: address_field(record.get("region"))?,
        postal_code: address_field(record.get("postalCode"))?,
        country: optional_address_field(record.get("country"))?,
    })
}

fn address_field(value: Option<&Value>) -> Result<String, OrderError> {
    let text = value.and_then(Value::as_str).unwrap_or("").trim();
    if text.is_empty() {
        return Err(OrderError::Invalid(
            "Enter the ship-to name, street, city, region, and postal code.",
        ));
    }
    if text.len() > 200 || text.as_bytes().contains(&0) {
        return Err(OrderError::Invalid("That address is too long."));
    }
    Ok(text.to_string())
}

fn optional_address_field(value: Option<&Value>) -> Result<String, OrderError> {
    let Some(value) = value else {
        return Ok(String::new());
    };
    let text = value.as_str().unwrap_or("").trim();
    if text.len() > 200 || text.as_bytes().contains(&0) {
        return Err(OrderError::Invalid("That address is too long."));
    }
    Ok(text.to_string())
}

fn format_ship_to(ship: ShipTo) -> String {
    let mut lines = vec![ship.name, ship.street];
    if !ship.street2.is_empty() {
        lines.push(ship.street2);
    }
    lines.push(format!(
        "{}, {} {}",
        ship.city, ship.region, ship.postal_code
    ));
    if !ship.country.is_empty() {
        lines.push(ship.country);
    }
    lines.join("\n")
}

fn client_named_destination(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, child)| destination_key(key) || client_named_destination(child)),
        Value::Array(items) => items.iter().any(client_named_destination),
        Value::String(text) => looks_like_peer_address(text),
        _ => false,
    }
}

fn destination_key(key: &str) -> bool {
    let folded: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    matches!(
        folded.as_str(),
        "destination"
            | "destinationurl"
            | "destinationuri"
            | "peer"
            | "peers"
            | "peeraddress"
            | "peerurl"
            | "vendorurl"
            | "vendoraddress"
            | "vendoronion"
            | "onion"
            | "onionaddress"
            | "seed"
            | "seeds"
            | "replica"
            | "replicas"
            | "nodeurl"
            | "endpoint"
            | "url"
            | "uri"
    )
}

fn looks_like_peer_address(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    lower.contains("://") || lower.contains(".onion")
}

fn delivery_note(value: Option<&Value>) -> Result<String, OrderError> {
    let text = value.and_then(Value::as_str).unwrap_or("").trim();
    if text.is_empty() {
        return Err(OrderError::Invalid(
            "Enter a delivery note or reply contact.",
        ));
    }
    if text.len() > 500 || text.as_bytes().contains(&0) {
        return Err(OrderError::Invalid("That note is too long."));
    }
    Ok(text.to_string())
}
