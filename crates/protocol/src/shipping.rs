//! How a listing ships: package size, shipping services, handling time, and
//! the opt-in local pickup area.
//!
//! Amounts are in the listing's currency, in minor units. Regions are country
//! codes only. A pickup area is a region name that passes
//! [`public_text::check_region`]; the meeting itself is arranged in private
//! order messages and never published.

use crate::encode::{Reader, Writer};
use crate::public_text;
use crate::store::CountryCode;
use crate::Error;

pub const MAX_SERVICES: usize = 6;
pub const MAX_SERVICE_REGIONS: usize = 32;
const MAX_SERVICE_NAME: usize = 40;
const MAX_PICKUP_AREA: usize = 40;
const MAX_HANDLING_DAYS: u8 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// Shipping for the first unit in a package.
    pub price: u64,
    /// Shipping for each further unit packed with it (combined shipping).
    pub extra_item: u64,
    pub days_min: u8,
    pub days_max: u8,
    /// Countries this service reaches. Empty means worldwide.
    pub regions: Vec<CountryCode>,
}

impl Service {
    pub fn reaches(&self, country: &CountryCode) -> bool {
        self.regions.is_empty() || self.regions.contains(country)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shipping {
    pub weight_g: u32,
    /// Length, width and height in millimetres.
    pub dims_mm: [u16; 3],
    pub handling_days: u8,
    /// Item subtotal at which shipping is free. Zero means never.
    pub free_over: u64,
    pub services: Vec<Service>,
    /// A coarse region where the seller will hand the item over in person.
    pub pickup: Option<String>,
}

impl Shipping {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub(crate) fn check(&self, digital: bool) -> Result<(), Error> {
        if digital && !self.is_empty() {
            return Err(Error::BadListing("a digital listing has no shipping"));
        }
        if self.handling_days > MAX_HANDLING_DAYS {
            return Err(Error::BadListing("handling time is at most 30 days"));
        }
        if self.services.len() > MAX_SERVICES {
            return Err(Error::BadListing("a listing has at most 6 shipping services"));
        }
        for (i, service) in self.services.iter().enumerate() {
            if service.name.trim().is_empty() || service.name.len() > MAX_SERVICE_NAME {
                return Err(Error::BadListing("a shipping service name is empty or too long"));
            }
            if self.services[..i].iter().any(|other| other.name.eq_ignore_ascii_case(&service.name)) {
                return Err(Error::BadListing("a shipping service name is used twice"));
            }
            if service.days_min > service.days_max {
                return Err(Error::BadListing("a delivery estimate ends before it starts"));
            }
            if service.extra_item > service.price {
                return Err(Error::BadListing("an extra item costs no more to ship than the first"));
            }
            if service.regions.len() > MAX_SERVICE_REGIONS || !service.regions.windows(2).all(|pair| pair[0] < pair[1]) {
                return Err(Error::BadListing("service regions are at most 32 sorted country codes"));
            }
        }
        if let Some(area) = &self.pickup {
            if area.trim().is_empty() || area.len() > MAX_PICKUP_AREA {
                return Err(Error::BadListing("a pickup area is empty or too long"));
            }
        }
        Ok(())
    }

    pub(crate) fn check_public_text(&self) -> Result<(), Error> {
        for service in &self.services {
            public_text::check("shipping service", &service.name)?;
        }
        if let Some(area) = &self.pickup {
            public_text::check_region("pickup area", area)?;
        }
        Ok(())
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.u32(self.weight_g);
        for dim in self.dims_mm {
            writer.u16(dim);
        }
        writer.u8(self.handling_days);
        writer.u64(self.free_over);
        writer.u8(self.services.len() as u8);
        for service in &self.services {
            writer.str(&service.name)?;
            writer.u64(service.price);
            writer.u64(service.extra_item);
            writer.u8(service.days_min);
            writer.u8(service.days_max);
            writer.u8(service.regions.len() as u8);
            for region in &service.regions {
                writer.bytes(region.as_str().as_bytes());
            }
        }
        match &self.pickup {
            None => writer.u8(0),
            Some(area) => {
                writer.u8(1);
                writer.str(area)?;
            }
        }
        Ok(())
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let weight_g = reader.u32()?;
        let dims_mm = [reader.u16()?, reader.u16()?, reader.u16()?];
        let handling_days = reader.u8()?;
        let free_over = reader.u64()?;
        let count = reader.u8()? as usize;
        if count > MAX_SERVICES {
            return Err(Error::BadListing("a listing has at most 6 shipping services"));
        }
        let mut services = Vec::with_capacity(count);
        for _ in 0..count {
            let name = reader.str()?;
            let price = reader.u64()?;
            let extra_item = reader.u64()?;
            let days_min = reader.u8()?;
            let days_max = reader.u8()?;
            let region_count = reader.u8()? as usize;
            if region_count > MAX_SERVICE_REGIONS {
                return Err(Error::BadListing("service regions are at most 32 sorted country codes"));
            }
            let mut regions = Vec::with_capacity(region_count);
            for _ in 0..region_count {
                let bytes: [u8; 2] = reader.array()?;
                let text = std::str::from_utf8(&bytes).map_err(|_| Error::BadString)?;
                regions.push(CountryCode::parse(text)?);
            }
            services.push(Service { name, price, extra_item, days_min, days_max, regions });
        }
        let pickup = match reader.u8()? {
            0 => None,
            1 => Some(reader.str()?),
            _ => return Err(Error::BadListing("unknown pickup tag")),
        };
        Ok(Self { weight_g, dims_mm, handling_days, free_over, services, pickup })
    }
}
