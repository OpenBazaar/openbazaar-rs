//! Which moderator one escrow uses.
//!
//! The buyer picks a moderator at checkout from the ones the store's signed
//! record accepts. Both sides then hold the same rule: the moderator's key is
//! in the store record the seller signed, this node holds a profile that key
//! signed, and the record in the escrow terms is the record in that profile.
//! The buyer checks the vendor's terms against the profile it chose
//! ([`Node::order_moderator_record`]); the vendor builds the terms from the
//! profile it holds ([`Node::sale_moderator`]).
//!
//! A request that names no moderator means the built-in test moderator. That
//! is the request older nodes send and understand, so the built-in path is
//! unchanged on the wire. A request for any other moderator carries its id
//! in a field older vendors do not read. So before anything is sealed the
//! buyer asks the vendor's node which moderators it takes
//! (`GET /public/moderators`) and refuses the checkout with the reason when
//! it does not take this one. The vendor checks again when the request
//! arrives; the question only makes the refusal visible to the buyer.

use std::time::Duration;

use openbazaar_escrow::{read_profile, ModeratorRecord, VerifiedProfile};
use serde_json::{json, Value};
use tormail::StoreKey;

use crate::escrow_fixture::moderator_record;
use crate::escrow_mail::{hex, key_bytes, unhex};
use crate::peer_http::sale_base;
use crate::{Node, OrderError};

const ASK_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY: usize = 256 * 1024;

impl Node {
    /// The moderator a buyer's checkout uses for `seller`: `chosen`, or the
    /// built-in one when none is named. Refused unless the store's signed
    /// record accepts it and, for any but the built-in one, this node holds
    /// its verified profile. `None` is the built-in moderator.
    pub(crate) fn checkout_moderator(&self, seller: &str, chosen: Option<[u8; 32]>) -> Result<Option<VerifiedProfile>, OrderError> {
        let accepted = self
            .seller_store(seller)
            .ok()
            .and_then(|store| store.details)
            .map(|details| details.moderators)
            .unwrap_or_default();
        let builtin = self.builtin_moderator();
        let id = chosen.or(builtin).ok_or(OrderError::Invalid(crate::instructions::ESCROW_MAINNET_REFUSAL))?;
        if !accepted.contains(&id) {
            return Err(OrderError::Invalid(match chosen {
                Some(_) => "This store's signed record does not accept that moderator.",
                None => "Choose one of the moderators this store accepts.",
            }));
        }
        if Some(id) == builtin {
            return Ok(None);
        }
        if Some(id) == self.own_moderator_id() {
            return Err(OrderError::Invalid("That moderator is this node. Choose another one."));
        }
        self.known_moderator(&id)
            .map(|known| Some(known.profile))
            .ok_or(OrderError::Invalid("This node has no verified profile for that moderator."))
    }

    /// Ask the vendor's node whether it takes the chosen moderator, before
    /// anything is sealed. `None` is the built-in one, which an older vendor
    /// node takes without being asked: for it, a node that cannot be reached
    /// or has no such route is not a refusal, and the request waits in the
    /// outbox as it always has.
    pub(crate) fn vendor_takes_moderator(&self, seller: &str, chosen: Option<&VerifiedProfile>) -> Result<(), OrderError> {
        let unconfirmed = |message: &'static str| match chosen {
            Some(_) => Err(OrderError::Invalid(message)),
            None => Ok(()),
        };
        let Some(id) = chosen.map(VerifiedProfile::id).or(self.builtin_moderator()) else {
            return Ok(());
        };
        let Ok(url) = self.vendor_url(seller) else {
            return unconfirmed("No node for this seller is set up to take escrow.");
        };
        let url = format!("{}/public/moderators", url.trim_end_matches('/'));
        let Ok(answer) = crate::peer_http::call(&self.peers, "GET", &url, None, None, ASK_TIMEOUT, MAX_BODY) else {
            return unconfirmed("The seller's node could not be reached to confirm that moderator. Try again, or choose another moderator.");
        };
        if answer.status == 404 {
            return unconfirmed("The seller's node runs an older build that only takes the built-in moderator. Choose that one, or pay directly.");
        }
        let answer = serde_json::from_slice::<Value>(&answer.body).ok().filter(|_| (200..300).contains(&answer.status));
        let (Some(accepted), Some(list)) = (
            answer.as_ref().and_then(|value| value["accepted"].as_array()),
            answer.as_ref().and_then(|value| value["moderators"].as_array()),
        ) else {
            return unconfirmed("The seller's node could not confirm that moderator. Try again, or choose another moderator.");
        };
        if !accepted.contains(&json!(hex(&id))) {
            return Err(OrderError::Invalid("The seller's node does not accept that moderator. Choose another one."));
        }
        let Some(profile) = chosen else {
            return Ok(());
        };
        let theirs = list
            .iter()
            .filter_map(|entry| read_profile(&unhex(entry.as_str()?).ok()?).ok())
            .find(|theirs| theirs.id() == profile.id());
        match theirs {
            Some(theirs) if theirs.record == profile.record => Ok(()),
            Some(theirs) if theirs.profile.updated > profile.profile.updated => {
                self.learn_moderators(&[json!(hex(&theirs.bytes))]);
                Err(OrderError::Invalid("That moderator changed its fee or policy. Review it at checkout and place the order again."))
            }
            Some(_) => Err(OrderError::Invalid(
                "The seller's node holds an older profile for that moderator, with another fee or policy. Choose another moderator.",
            )),
            None => Err(OrderError::Invalid("The seller's node holds no profile for that moderator. Choose another one.")),
        }
    }

    /// The record of the moderator a new order uses, and what the order's
    /// escrow record says about it. The built-in moderator (`None`) is kept
    /// without a profile: its key is public, so a profile proves nothing.
    pub(crate) fn order_moderator(&self, chosen: Option<&VerifiedProfile>) -> Result<(ModeratorRecord, Value), String> {
        match chosen {
            Some(profile) => Ok((
                profile.record.clone(),
                json!({ "id": hex(&profile.id()), "name": profile.profile.name, "builtIn": false, "profile": hex(&profile.bytes) }),
            )),
            None => {
                let record = moderator_record(self.network)?;
                let name = self.catalog().moderator.name.clone();
                let value = json!({ "id": hex(&record.id()), "name": name, "builtIn": true });
                Ok((record, value))
            }
        }
    }

    /// The profile the buyer chose for this order, checked again. `None` for
    /// the built-in moderator, which every order from before a moderator
    /// could be chosen uses.
    fn order_profile(escrow: &Value) -> Result<Option<VerifiedProfile>, String> {
        let Some(text) = escrow["moderator"]["profile"].as_str() else {
            return Ok(None);
        };
        read_profile(&unhex(text)?).map(Some).map_err(|_| "this order's moderator profile does not verify".to_string())
    }

    /// The moderator record this order's terms must carry: the one in the
    /// profile the buyer chose, or the built-in one for an older order.
    pub(crate) fn order_moderator_record(&self, escrow: &Value) -> Result<ModeratorRecord, String> {
        match Self::order_profile(escrow)? {
            Some(profile) => Ok(profile.record),
            None => moderator_record(self.network),
        }
    }

    /// Where the buyer's dispute letter for this order goes.
    pub(crate) fn order_moderator_mailbox(&self, escrow: &Value) -> Result<(String, StoreKey), String> {
        let stored = Self::order_profile(escrow)?;
        let record = self.order_moderator_record(escrow)?;
        self.moderator_mailbox(&record, stored)
    }

    /// The address and mailbox key of the moderator whose record is
    /// `record`: from the newest profile this node holds for that key, else `stored`.
    pub(crate) fn moderator_mailbox(&self, record: &ModeratorRecord, stored: Option<VerifiedProfile>) -> Result<(String, StoreKey), String> {
        let profile = self
            .known_moderator(&record.id())
            .map(|known| known.profile)
            .or(stored)
            .ok_or("This node has no moderator to send a dispute to.")?;
        Ok((sale_base(&profile.profile.address)?, key_bytes(&profile.profile.mailbox_key)?))
    }

    /// The record a vendor puts in the terms for `seller`'s sale: the
    /// built-in moderator's when the request names none, else the one in the
    /// profile this node holds for `chosen`. Refused unless the store's own
    /// signed record accepts that moderator.
    pub(crate) fn sale_moderator(&self, seller: &str, chosen: Option<[u8; 32]>) -> Result<ModeratorRecord, String> {
        let builtin = moderator_record(self.network)?;
        let id = chosen.unwrap_or(builtin.id());
        let accepted = self.seller_store(seller)?.details.map(|details| details.moderators).unwrap_or_default();
        if !accepted.contains(&id) {
            return Err("this store's signed record does not accept that moderator".into());
        }
        if id == builtin.id() {
            return Ok(builtin);
        }
        if Some(id) == self.own_moderator_id() {
            return Err("a store cannot use its own node as its moderator".into());
        }
        self.known_moderator(&id)
            .map(|known| known.profile.record)
            .ok_or_else(|| "this node holds no verified profile for that moderator".into())
    }
}
