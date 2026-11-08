//! The `commerce` part of a listing draft: condition, item specifics,
//! options and variants, pricing rules, purchase terms, shipping and returns.
//!
//! The editor sends it as JSON next to the version 6 fields; this module
//! turns it into the signed [`Commerce`] block and back. A variant keeps the
//! id the editor sends back for it, so orders keep finding it across edits;
//! a variant without one gets a fresh random id. Range and privacy checks
//! that belong to the record run again when the listing is signed.

use std::collections::BTreeMap;

use openbazaar_protocol::{
    Availability, Axis, Commerce, Condition, CountryCode, Pricing, Purchase, Returns, Sale, Service, Shipping, Tier,
    Variant, VariantId, Variants,
};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};

const FIELDS: &[&str] = &[
    "condition",
    "conditionNote",
    "taxonomy",
    "specifics",
    "axes",
    "variants",
    "compareAt",
    "sale",
    "tiers",
    "acceptsOffers",
    "maxPerOrder",
    "lotSize",
    "availability",
    "shipping",
    "returns",
];

type Result<T> = std::result::Result<T, String>;

fn fail<T>(message: &str) -> Result<T> {
    Err(message.to_string())
}

fn uint(value: &Value, what: &str, max: u64) -> Result<u64> {
    match value {
        Value::Null => Ok(0),
        value => value.as_u64().filter(|n| *n <= max).ok_or_else(|| format!("The {what} must be a whole number up to {max}.")),
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").trim().to_string()
}

/// Parse `body`, the draft's `commerce` object. `previous` is the version
/// being edited, whose variant ids may be reused.
pub(crate) fn parse(body: &Value, previous: Option<&Commerce>) -> Result<Commerce> {
    let Some(map) = body.as_object() else {
        return fail("The listing's options could not be read.");
    };
    if let Some(key) = map.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(format!("\"{key}\" is not part of a listing's options."));
    }
    let condition = match body["condition"].as_str() {
        None => Condition::New,
        Some(name) => Condition::parse(name).ok_or("Choose a condition: new, open-box, like-new, good, fair, or for-parts.")?,
    };
    Ok(Commerce {
        condition,
        condition_note: text(&body["conditionNote"]),
        taxonomy: uint(&body["taxonomy"], "category number", u64::from(u16::MAX))? as u16,
        specifics: specifics(&body["specifics"])?,
        variants: variants(&body["axes"], &body["variants"], previous)?,
        pricing: Pricing {
            compare_at: uint(&body["compareAt"], "compare-at price", u64::MAX)?,
            sale: sale(&body["sale"])?,
            tiers: tiers(&body["tiers"])?,
            accepts_offers: match &body["acceptsOffers"] {
                Value::Null => false,
                Value::Bool(flag) => *flag,
                _ => return fail("An offers flag is yes or no."),
            },
        },
        purchase: Purchase {
            max_per_order: uint(&body["maxPerOrder"], "per-order limit", u64::from(u16::MAX))? as u16,
            lot_size: match uint(&body["lotSize"], "lot size", u64::from(u16::MAX))? {
                0 => 1,
                n => n as u16,
            },
            availability: availability(&body["availability"])?,
        },
        shipping: shipping(&body["shipping"])?,
        returns: returns(&body["returns"])?,
    })
}

/// Item specifics as an object of key to value. Keys are normalised to the
/// record's lowercase-and-dashes form; empty values are dropped.
fn specifics(value: &Value) -> Result<Vec<(String, String)>> {
    let mut out = BTreeMap::new();
    for (key, value) in value.as_object().into_iter().flatten() {
        let key = key.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        let value = text(value);
        if !value.is_empty() {
            out.insert(key, value);
        }
    }
    Ok(out.into_iter().collect())
}

fn variants(axes: &Value, rows: &Value, previous: Option<&Commerce>) -> Result<Variants> {
    let axes: Vec<Axis> = axes
        .as_array()
        .into_iter()
        .flatten()
        .map(|axis| Axis {
            name: text(&axis["name"]),
            values: axis["values"].as_array().into_iter().flatten().map(text).filter(|v| !v.is_empty()).collect(),
        })
        .filter(|axis| !axis.name.is_empty() || !axis.values.is_empty())
        .collect();
    let known: Vec<VariantId> = previous.map(|c| c.variants.variants.iter().map(|v| v.id).collect()).unwrap_or_default();
    let mut variants = Vec::new();
    for row in rows.as_array().into_iter().flatten() {
        let options = row["options"].as_array().ok_or("Each variant names one value per option.")?;
        if options.len() != axes.len() {
            return fail("Each variant names one value per option.");
        }
        let mut choice = Vec::with_capacity(axes.len());
        for (axis, option) in axes.iter().zip(options) {
            let wanted = text(option);
            let index = axis
                .values
                .iter()
                .position(|value| value.eq_ignore_ascii_case(&wanted))
                .ok_or_else(|| format!("\"{wanted}\" is not one of the {} values.", axis.name))?;
            choice.push(index as u8);
        }
        let id = match row["id"].as_str().and_then(variant_id_of) {
            Some(bytes) if known.contains(&VariantId(bytes)) => VariantId(bytes),
            _ => new_variant_id(),
        };
        variants.push(Variant {
            id,
            choice,
            sku: text(&row["sku"]),
            price: uint(&row["price"], "variant price", u64::MAX)?,
            stock: uint(&row["stock"], "variant stock", 1_000_000)? as u32,
            image: uint(&row["image"], "variant image", 255)? as u8,
            weight_g: uint(&row["weightG"], "variant weight", u64::from(u32::MAX))? as u32,
        });
    }
    Ok(Variants { axes, variants })
}

pub(crate) fn variant_id_of(text: &str) -> Option<[u8; 8]> {
    crate::escrow_mail::unhex(text).ok()?.try_into().ok()
}

fn new_variant_id() -> VariantId {
    loop {
        let mut bytes = [0u8; 8];
        StdRng::from_os_rng().fill_bytes(&mut bytes);
        if bytes != [0; 8] {
            return VariantId(bytes);
        }
    }
}

fn day(value: &Value, what: &str) -> Result<u32> {
    let day = uint(value, what, 99_991_231)? as u32;
    if day < 20_000_101 || !(1..=12).contains(&(day / 100 % 100)) || !(1..=31).contains(&(day % 100)) {
        return Err(format!("The {what} must be a date written YYYYMMDD."));
    }
    Ok(day)
}

fn sale(value: &Value) -> Result<Option<Sale>> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(Sale {
        off_bps: uint(&value["offBps"], "sale discount", 9_000)? as u16,
        starts_on: day(&value["startsOn"], "sale start")?,
        ends_on: day(&value["endsOn"], "sale end")?,
    }))
}

fn tiers(value: &Value) -> Result<Vec<Tier>> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|tier| {
            Ok(Tier {
                min_qty: uint(&tier["minQty"], "tier quantity", u64::from(u16::MAX))? as u16,
                off_bps: uint(&tier["offBps"], "tier discount", 9_000)? as u16,
            })
        })
        .collect()
}

fn availability(value: &Value) -> Result<Availability> {
    Ok(match value["kind"].as_str().unwrap_or("in-stock") {
        "in-stock" => Availability::InStock,
        "pre-order" => Availability::PreOrder { ships_on: day(&value["shipsOn"], "pre-order ship date")? },
        "made-to-order" => Availability::MadeToOrder { days: uint(&value["days"], "making time", 365)? as u16 },
        "away" => Availability::Away,
        _ => return fail("Availability is in-stock, pre-order, made-to-order, or away."),
    })
}

fn shipping(value: &Value) -> Result<Shipping> {
    if value.is_null() {
        return Ok(Shipping::default());
    }
    let dims = value["dimsMm"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let dim = |i: usize| uint(dims.get(i).unwrap_or(&Value::Null), "package size", u64::from(u16::MAX)).map(|n| n as u16);
    let mut services = Vec::new();
    for service in value["services"].as_array().into_iter().flatten() {
        let mut regions = Vec::new();
        for region in service["regions"].as_array().into_iter().flatten() {
            let code = CountryCode::parse(&text(region).to_ascii_uppercase()).map_err(|_| "A region is a two-letter country code.")?;
            regions.push(code);
        }
        regions.sort();
        regions.dedup();
        services.push(Service {
            name: text(&service["name"]),
            price: uint(&service["price"], "shipping price", u64::MAX)?,
            extra_item: uint(&service["extraItem"], "extra-item shipping", u64::MAX)?,
            days_min: uint(&service["daysMin"], "delivery estimate", 255)? as u8,
            days_max: uint(&service["daysMax"], "delivery estimate", 255)? as u8,
            regions,
        });
    }
    let pickup = Some(text(&value["pickup"])).filter(|area| !area.is_empty());
    Ok(Shipping {
        weight_g: uint(&value["weightG"], "package weight", u64::from(u32::MAX))? as u32,
        dims_mm: [dim(0)?, dim(1)?, dim(2)?],
        handling_days: uint(&value["handlingDays"], "handling time", 30)? as u8,
        free_over: uint(&value["freeOver"], "free-shipping threshold", u64::MAX)?,
        services,
        pickup,
    })
}

fn returns(value: &Value) -> Result<Returns> {
    Ok(match value["kind"].as_str().unwrap_or("store") {
        "store" => Returns::Store,
        "none" => Returns::NoReturns,
        "days" => Returns::Days {
            days: uint(&value["days"], "returns window", 365)? as u16,
            seller_pays: value["sellerPays"].as_bool().unwrap_or(false),
        },
        _ => return fail("Returns are the store's policy, none, or a number of days."),
    })
}

/// The block as JSON, in the shape [`parse`] reads.
pub(crate) fn value(commerce: &Commerce) -> Value {
    let variants = &commerce.variants;
    let availability = match commerce.purchase.availability {
        Availability::InStock => json!({ "kind": "in-stock" }),
        Availability::PreOrder { ships_on } => json!({ "kind": "pre-order", "shipsOn": ships_on }),
        Availability::MadeToOrder { days } => json!({ "kind": "made-to-order", "days": days }),
        Availability::Away => json!({ "kind": "away" }),
    };
    let returns = match commerce.returns {
        Returns::Store => json!({ "kind": "store" }),
        Returns::NoReturns => json!({ "kind": "none" }),
        Returns::Days { days, seller_pays } => json!({ "kind": "days", "days": days, "sellerPays": seller_pays }),
    };
    let shipping = &commerce.shipping;
    json!({
        "condition": commerce.condition.as_str(),
        "conditionNote": commerce.condition_note,
        "taxonomy": commerce.taxonomy,
        "specifics": commerce.specifics.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect::<serde_json::Map<_, _>>(),
        "axes": variants.axes.iter().map(|axis| json!({ "name": axis.name, "values": axis.values })).collect::<Vec<_>>(),
        "variants": variants.variants.iter().map(|variant| json!({
            "id": variant.id.to_hex(),
            "options": variants.describe(variant).into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
            "sku": variant.sku,
            "price": variant.price,
            "stock": variant.stock,
            "image": variant.image,
            "weightG": variant.weight_g,
        })).collect::<Vec<_>>(),
        "compareAt": commerce.pricing.compare_at,
        "sale": commerce.pricing.sale.map(|sale| json!({ "offBps": sale.off_bps, "startsOn": sale.starts_on, "endsOn": sale.ends_on })),
        "tiers": commerce.pricing.tiers.iter().map(|tier| json!({ "minQty": tier.min_qty, "offBps": tier.off_bps })).collect::<Vec<_>>(),
        "acceptsOffers": commerce.pricing.accepts_offers,
        "maxPerOrder": commerce.purchase.max_per_order,
        "lotSize": commerce.purchase.lot_size,
        "availability": availability,
        "shipping": {
            "weightG": shipping.weight_g,
            "dimsMm": shipping.dims_mm,
            "handlingDays": shipping.handling_days,
            "freeOver": shipping.free_over,
            "services": shipping.services.iter().map(|service| json!({
                "name": service.name,
                "price": service.price,
                "extraItem": service.extra_item,
                "daysMin": service.days_min,
                "daysMax": service.days_max,
                "regions": service.regions.iter().map(CountryCode::as_str).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "pickup": shipping.pickup,
        },
        "returns": returns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Value {
        json!({
            "condition": "like-new",
            "conditionNote": "Worn twice.",
            "taxonomy": 3,
            "specifics": { "Brand": "Marlow", "material": "Cotton", "upc": "" },
            "axes": [{ "name": "Size", "values": ["S", "M"] }, { "name": "Color", "values": ["Navy"] }],
            "variants": [
                { "options": ["S", "Navy"], "sku": "T-S", "stock": 3 },
                { "options": ["m", "navy"], "sku": "T-M", "price": 2600, "stock": 1 }
            ],
            "compareAt": 3200,
            "sale": { "offBps": 1500, "startsOn": 20261101, "endsOn": 20261130 },
            "tiers": [{ "minQty": 3, "offBps": 1000 }],
            "acceptsOffers": true,
            "maxPerOrder": 4,
            "availability": { "kind": "pre-order", "shipsOn": 20261201 },
            "shipping": {
                "weightG": 250, "dimsMm": [300, 200, 30], "handlingDays": 2, "freeOver": 10000,
                "services": [{ "name": "Standard", "price": 500, "extraItem": 100, "daysMin": 2, "daysMax": 5, "regions": ["us", "CA"] }],
                "pickup": "Oregon"
            },
            "returns": { "kind": "days", "days": 30, "sellerPays": true }
        })
    }

    #[test]
    fn a_draft_parses_and_prints_back() {
        let commerce = parse(&draft(), None).unwrap();
        assert_eq!(commerce.condition, Condition::LikeNew);
        assert_eq!(commerce.specifics, vec![("brand".into(), "Marlow".into()), ("material".into(), "Cotton".into())]);
        assert_eq!(commerce.variants.variants[1].choice, vec![1, 0]);
        assert_eq!(commerce.shipping.services[0].regions.iter().map(CountryCode::as_str).collect::<Vec<_>>(), ["CA", "US"]);
        assert_eq!(commerce.purchase.lot_size, 1);
        let again = parse(&value(&commerce), Some(&commerce)).unwrap();
        assert_eq!(again, commerce, "ids and every field survive a round trip through the editor");
    }

    #[test]
    fn unknown_ids_get_fresh_ones_and_known_ids_stay() {
        let first = parse(&draft(), None).unwrap();
        let mut edited = value(&first);
        edited["variants"][0]["id"] = json!("0101010101010101");
        edited["variants"][1]["stock"] = json!(9);
        let second = parse(&edited, Some(&first)).unwrap();
        assert_ne!(second.variants.variants[0].id, VariantId([1; 8]));
        assert_eq!(second.variants.variants[1].id, first.variants.variants[1].id);
        assert_eq!(second.variants.variants[1].stock, 9);
    }

    #[test]
    fn mistakes_read_as_sentences() {
        let mut bad = draft();
        bad["variants"][0]["options"] = json!(["XL", "Navy"]);
        assert_eq!(parse(&bad, None).unwrap_err(), "\"XL\" is not one of the Size values.");
        let mut bad = draft();
        bad["sale"]["endsOn"] = json!(20261340);
        assert_eq!(parse(&bad, None).unwrap_err(), "The sale end must be a date written YYYYMMDD.");
        let mut bad = draft();
        bad["surprise"] = json!(1);
        assert!(parse(&bad, None).is_err());
    }
}
