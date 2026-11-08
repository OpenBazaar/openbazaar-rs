//! Listings the seller has not published.
//!
//! The editor's JSON is kept in `listing-drafts.obv`, sealed with the vault
//! key. A draft is not signed and is not served in the catalog. Publishing
//! uses the existing listing sign path, then the draft can be dropped.

use std::path::{Path, PathBuf};

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};

use crate::escrow_mail::hex;
use crate::own_listings::StoreError;
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "listing-drafts.obv";
const MAX_DRAFTS: usize = 100;

struct Draft {
    id: String,
    listing: Value,
}

pub(crate) struct ListingDrafts {
    path: PathBuf,
    vault: Vault,
    drafts: Vec<Draft>,
}

impl ListingDrafts {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut drafts = Vec::new();
        if let Some(bytes) = vault.read_sealed(&path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
            for item in value["drafts"].as_array().into_iter().flatten() {
                let Some(id) = item["draftId"].as_str().map(str::to_string) else { continue };
                if !item["listing"].is_object() {
                    continue;
                }
                drafts.push(Draft { id, listing: item["listing"].clone() });
            }
        }
        Ok(Self { path, vault, drafts })
    }

    fn save(&self) -> Result<(), StoreError> {
        let drafts: Vec<Value> = self.drafts.iter().map(|draft| json!({ "draftId": draft.id, "listing": draft.listing })).collect();
        let bytes = serde_json::to_vec(&json!({ "v": 1, "drafts": drafts })).map_err(|err| StoreError::Failed(err.to_string()))?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| StoreError::Failed(err.to_string()))
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 8];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    hex(&bytes)
}

fn listing_of(body: &Value) -> Result<Value, StoreError> {
    let mut listing = body.as_object().cloned().ok_or_else(|| StoreError::Invalid("That draft could not be read.".into()))?;
    listing.remove("draftId");
    Ok(Value::Object(listing))
}

fn draft_value(draft: &Draft) -> Value {
    json!({ "draftId": draft.id, "listing": draft.listing })
}

impl Node {
    pub(crate) fn listing_drafts_value(&self) -> Result<Value, StoreError> {
        let drafts = self.listing_drafts.lock().unwrap();
        Ok(json!({ "drafts": drafts.drafts.iter().map(draft_value).collect::<Vec<_>>() }))
    }

    /// Keep `body` unpublished. `id` updates that draft; otherwise this is a new one.
    pub(crate) fn save_listing_draft(&self, id: Option<&str>, body: &Value) -> Result<Value, StoreError> {
        let listing = listing_of(body)?;
        let mut drafts = self.listing_drafts.lock().unwrap();
        if let Some(id) = id {
            let draft = drafts.drafts.iter_mut().find(|draft| draft.id == id).ok_or(StoreError::NotFound)?;
            let before = draft.listing.clone();
            draft.listing = listing;
            let value = draft_value(draft);
            if let Err(err) = drafts.save() {
                if let Some(draft) = drafts.drafts.iter_mut().find(|draft| draft.id == id) {
                    draft.listing = before;
                }
                return Err(err);
            }
            return Ok(value);
        }
        if drafts.drafts.len() >= MAX_DRAFTS {
            return Err(StoreError::Invalid("A store keeps at most 100 drafts.".into()));
        }
        drafts.drafts.push(Draft { id: new_id(), listing });
        let value = draft_value(drafts.drafts.last().unwrap());
        if let Err(err) = drafts.save() {
            drafts.drafts.pop();
            return Err(err);
        }
        Ok(value)
    }

    pub(crate) fn delete_listing_draft(&self, id: &str) -> Result<(), StoreError> {
        let mut drafts = self.listing_drafts.lock().unwrap();
        let index = drafts.drafts.iter().position(|draft| draft.id == id).ok_or(StoreError::NotFound)?;
        let removed = drafts.drafts.remove(index);
        if let Err(err) = drafts.save() {
            drafts.drafts.insert(index, removed);
            return Err(err);
        }
        Ok(())
    }
}
