//! Top-level categories a version 7 listing can name by number.
//!
//! The numbers are part of signed records and never change meaning. A client
//! that meets a number it does not know shows the listing under "Other". The
//! listing's free-text category stays as the subcategory.

pub const TAXONOMY: &[(u16, &str)] = &[
    (1, "Books"),
    (2, "Art and collectibles"),
    (3, "Clothing and shoes"),
    (4, "Electronics"),
    (5, "Home and kitchen"),
    (6, "Toys and games"),
    (7, "Sports and outdoors"),
    (8, "Health and beauty"),
    (9, "Jewelry and accessories"),
    (10, "Music and media"),
    (11, "Tools and supplies"),
    (12, "Digital goods"),
    (13, "Food and drink"),
    (14, "Vehicles and parts"),
    (15, "Other"),
];

pub fn taxonomy_name(id: u16) -> &'static str {
    TAXONOMY.iter().find(|(known, _)| *known == id).map_or("Other", |(_, name)| name)
}
