//! Option axes (size, colour, material...) and the variants a listing offers.
//!
//! A variant is one combination of axis values with its own id, SKU, price,
//! stock, image and weight. Combinations the seller leaves out are not
//! offered. Variant ids are random and kept across edits, so an order can
//! name the variant it bought after the listing has been re-signed.

use crate::encode::{self, Reader, Writer};
use crate::public_text;
use crate::Error;

pub const MAX_AXES: usize = 3;
pub const MAX_AXIS_VALUES: usize = 16;
pub const MAX_VARIANTS: usize = 100;
const MAX_AXIS_NAME: usize = 32;
const MAX_VALUE: usize = 40;
const MAX_SKU: usize = 40;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VariantId(pub [u8; 8]);

impl VariantId {
    /// Stands for "no variant" in an order for a listing without axes.
    pub const NONE: VariantId = VariantId([0; 8]);

    pub fn to_hex(self) -> String {
        encode::to_hex(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Axis {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub id: VariantId,
    /// Index into each axis's values, one per axis.
    pub choice: Vec<u8>,
    pub sku: String,
    /// Unit price in the listing's currency. Zero means the listing's price.
    pub price: u64,
    pub stock: u32,
    /// Zero means the listing's cover; otherwise a 1-based index into its images.
    pub image: u8,
    /// Zero means the package weight.
    pub weight_g: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Variants {
    pub axes: Vec<Axis>,
    pub variants: Vec<Variant>,
}

impl Variants {
    pub fn is_empty(&self) -> bool {
        self.axes.is_empty()
    }

    pub fn get(&self, id: VariantId) -> Option<&Variant> {
        self.variants.iter().find(|variant| variant.id == id)
    }

    /// `(axis name, value)` for each axis of `variant`, for a packing slip or a label.
    pub fn describe(&self, variant: &Variant) -> Vec<(String, String)> {
        self.axes
            .iter()
            .zip(&variant.choice)
            .map(|(axis, &index)| (axis.name.clone(), axis.values[index as usize].clone()))
            .collect()
    }

    pub fn total_stock(&self) -> u64 {
        self.variants.iter().map(|variant| u64::from(variant.stock)).sum()
    }

    /// Structural checks; `images` is how many images the listing names.
    pub(crate) fn check(&self, images: usize) -> Result<(), Error> {
        if self.axes.len() > MAX_AXES {
            return Err(Error::BadListing("a listing has at most 3 option axes"));
        }
        if self.axes.is_empty() != self.variants.is_empty() {
            return Err(Error::BadListing("option axes and variants come together"));
        }
        if self.variants.len() > MAX_VARIANTS {
            return Err(Error::BadListing("a listing has at most 100 variants"));
        }
        for (i, axis) in self.axes.iter().enumerate() {
            check_label(&axis.name, MAX_AXIS_NAME, "an option name is empty or too long")?;
            if self.axes[..i].iter().any(|other| other.name.eq_ignore_ascii_case(&axis.name)) {
                return Err(Error::BadListing("an option name is used twice"));
            }
            if axis.values.is_empty() || axis.values.len() > MAX_AXIS_VALUES {
                return Err(Error::BadListing("an option has 1 to 16 values"));
            }
            for (j, value) in axis.values.iter().enumerate() {
                check_label(value, MAX_VALUE, "an option value is empty or too long")?;
                if axis.values[..j].iter().any(|other| other.eq_ignore_ascii_case(value)) {
                    return Err(Error::BadListing("an option value is used twice"));
                }
            }
        }
        for (i, variant) in self.variants.iter().enumerate() {
            if variant.id == VariantId::NONE {
                return Err(Error::BadListing("a variant id is not zero"));
            }
            if variant.choice.len() != self.axes.len() {
                return Err(Error::BadListing("a variant picks one value per option"));
            }
            if variant.choice.iter().zip(&self.axes).any(|(&index, axis)| index as usize >= axis.values.len()) {
                return Err(Error::BadListing("a variant picks a value that is not offered"));
            }
            if variant.sku.len() > MAX_SKU {
                return Err(Error::BadListing("a SKU is at most 40 bytes"));
            }
            if variant.image as usize > images {
                return Err(Error::BadListing("a variant names an image the listing does not have"));
            }
            for other in &self.variants[..i] {
                if other.id == variant.id {
                    return Err(Error::BadListing("a variant id is used twice"));
                }
                if other.choice == variant.choice {
                    return Err(Error::BadListing("two variants pick the same options"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn check_public_text(&self) -> Result<(), Error> {
        for axis in &self.axes {
            public_text::check("option name", &axis.name)?;
            for value in &axis.values {
                public_text::check("option value", value)?;
            }
        }
        for variant in &self.variants {
            public_text::check("SKU", &variant.sku)?;
        }
        Ok(())
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.u8(self.axes.len() as u8);
        for axis in &self.axes {
            writer.str(&axis.name)?;
            writer.u8(axis.values.len() as u8);
            for value in &axis.values {
                writer.str(value)?;
            }
        }
        writer.u8(self.variants.len() as u8);
        for variant in &self.variants {
            writer.bytes(&variant.id.0);
            writer.bytes(&variant.choice);
            writer.str(&variant.sku)?;
            writer.u64(variant.price);
            writer.u32(variant.stock);
            writer.u8(variant.image);
            writer.u32(variant.weight_g);
        }
        Ok(())
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let axis_count = reader.u8()? as usize;
        if axis_count > MAX_AXES {
            return Err(Error::BadListing("a listing has at most 3 option axes"));
        }
        let mut axes = Vec::with_capacity(axis_count);
        for _ in 0..axis_count {
            let name = reader.str()?;
            let count = reader.u8()? as usize;
            if count > MAX_AXIS_VALUES {
                return Err(Error::BadListing("an option has 1 to 16 values"));
            }
            let values = (0..count).map(|_| reader.str()).collect::<Result<_, _>>()?;
            axes.push(Axis { name, values });
        }
        let variant_count = reader.u8()? as usize;
        if variant_count > MAX_VARIANTS {
            return Err(Error::BadListing("a listing has at most 100 variants"));
        }
        let mut variants = Vec::with_capacity(variant_count);
        for _ in 0..variant_count {
            let id = VariantId(reader.array()?);
            let choice = (0..axis_count).map(|_| reader.u8()).collect::<Result<_, _>>()?;
            variants.push(Variant {
                id,
                choice,
                sku: reader.str()?,
                price: reader.u64()?,
                stock: reader.u32()?,
                image: reader.u8()?,
                weight_g: reader.u32()?,
            });
        }
        Ok(Self { axes, variants })
    }
}

fn check_label(text: &str, max: usize, message: &'static str) -> Result<(), Error> {
    if text.trim().is_empty() || text.len() > max || text.trim() != text {
        return Err(Error::BadListing(message));
    }
    Ok(())
}
