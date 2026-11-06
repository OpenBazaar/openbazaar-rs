//! A sample labeler for the fixture shop, so safe mode has something to act on.
//!
//! Nobody follows it by default. Its key is public, like the other fixture keys.

use bitcoin::secp256k1::SecretKey;

use crate::content::{ContentLabel, ContentRating, ContentTags};
use crate::fixtures::fixture_catalog;
use crate::labeler::{LabelEntry, LabelSet, LabelTarget};
use crate::sign;

const LABELER_KEY: &str = "openbazaar/phase0/fixture/labeler";
const NAME: &str = "Sample labeler";

pub fn fixture_labeler_key() -> SecretKey {
    sign::secret_from_label(LABELER_KEY)
}

/// Two sets: the first labels three tools, the second retracts one of them
/// and labels the coffee.
pub fn fixture_label_sets() -> Vec<LabelSet> {
    let secret = fixture_labeler_key();
    let first = vec![
        label("Kitchen shears", "North Kitchen", ContentRating::Sensitive, ContentTags::WEAPONS, "blade"),
        label("Sewing shears", "Hale Paper", ContentRating::Sensitive, ContentTags::WEAPONS, "blade"),
        label("Garden trowel", "Hale Paper", ContentRating::Sensitive, ContentTags::WEAPONS, "blade"),
    ];
    let second = vec![
        LabelEntry {
            target: target("Garden trowel", "Hale Paper"),
            label: None,
            reason: String::new(),
        },
        label("Whole bean coffee, 12 oz", "North Kitchen", ContentRating::Sensitive, ContentTags::DRUGS, "stimulant"),
    ];
    vec![
        LabelSet::sign(NAME, 1, 20261101, first, &secret).expect("fixture label set signs"),
        LabelSet::sign(NAME, 2, 20261106, second, &secret).expect("fixture label set signs"),
    ]
}

fn label(title: &str, seller: &str, rating: ContentRating, tags: ContentTags, reason: &str) -> LabelEntry {
    LabelEntry {
        target: target(title, seller),
        label: Some(ContentLabel::new(rating, tags)),
        reason: reason.to_string(),
    }
}

fn target(title: &str, seller: &str) -> LabelTarget {
    let listing = fixture_catalog()
        .listings
        .into_iter()
        .find(|listing| listing.title == title && listing.seller == seller)
        .unwrap_or_else(|| panic!("fixture listing {title:?} from {seller:?}"));
    LabelTarget::Listing(listing.content_hash().expect("fixture listing encodes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labeler::resolve_labels;

    #[test]
    fn fixture_sets_verify_and_resolve_to_the_tools_and_coffee() {
        let sets = fixture_label_sets();
        for set in &sets {
            set.verify().unwrap();
            assert_eq!(set.labeler, sign::xonly(&fixture_labeler_key()));
        }
        let labels = resolve_labels(&sets);
        let mut reasons: Vec<&str> = labels.iter().map(|label| label.reason.as_str()).collect();
        reasons.sort();
        assert_eq!(reasons, ["blade", "blade", "stimulant"]);
        assert!(!labels.iter().any(|label| label.target == target("Garden trowel", "Hale Paper")));
    }
}
