//! The fixture moderator's desk (testnet4 only, `EscrowConfig::moderates`).
//!
//! A dispute letter, from the buyer or the vendor, is reviewed when it
//! arrives: the escrow crate checks the terms, the claimant's signature, the
//! deadline against this node's tip, the rate limits ([`DisputeLimits`]),
//! the funding output against this node's chain source, and the proofs. The
//! case then waits for the operator, who rules with
//! `POST /moderator/disputes/{id}/rule`. A letter can only be ruled for the
//! branch it asks for.
//!
//! The chain check follows the funding script through
//! [`ChainSource::script_txs`] rather than looking the txid up, so it works
//! on compact block filters, which cannot look a transaction up by txid.
//! While the filters are still scanning for that script the letter stays in
//! the inbox and is reviewed again.
//!
//! Cases are durable records ([`crate::escrow_store`]), one per escrow and
//! branch. Rulings go through [`FileRulingStore`], which records at most one
//! branch per escrow across restarts and crashes before anything is
//! decrypted. A ruling is stored with its case before it is sent, so a crash
//! after the ruling resends the same branch.

use std::collections::BTreeMap;
use std::sync::MutexGuard;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{schnorr, Secp256k1, XOnlyPublicKey};
use bitcoin::{OutPoint, Script, TxOut};
use openbazaar_escrow::{Error as EscrowError, FundingChain, Moderator, ModeratorRuling, Party};
use serde_json::{json, Value};
use tormail::StoreKey;

use crate::chain::ChainSource;
use crate::escrow_fixture::{moderator_keys, moderator_record};
use crate::escrow_vendor::offer_digest_with_listing;
use crate::escrow_limits::DisputeLimits;
use crate::escrow_mail::{hex, id_hex, key_bytes, parse_id, unhex, EscrowId, Kind, Mail, RETRY_LATER};
use crate::escrow_outbox::now_secs;
use crate::escrow_rulings::FileRulingStore;
use crate::escrow_store::{EscrowStore, Role};
use crate::pay_order::PayError;
use crate::Node;

#[derive(Clone, Copy)]
struct Citation {
    listing_id: [u8; 16],
    store_key: [u8; 32],
    store_signature: [u8; 64],
}

struct Case {
    letter: Vec<u8>,
    memo: String,
    outpoint: OutPoint,
    received: u32,
    reply_mail: StoreKey,
    reply_url: String,
    ruling: Option<Vec<u8>>,
    citation: Option<Citation>,
    public_ruling: Option<Vec<u8>>,
}

impl Case {
    fn to_record(&self) -> Value {
        json!({
            "letter": hex(&self.letter),
            "memo": self.memo,
            "outpoint": self.outpoint.to_string(),
            "received": self.received,
            "replyMail": hex(self.reply_mail.as_bytes()),
            "replyUrl": self.reply_url,
            "ruling": self.ruling.as_deref().map(hex),
            "listingId": self.citation.map(|citation| hex(&citation.listing_id)),
            "storeKey": self.citation.map(|citation| hex(&citation.store_key)),
            "storeSignature": self.citation.map(|citation| hex(&citation.store_signature)),
            "publicRuling": self.public_ruling.as_deref().map(hex),
        })
    }

    fn from_record(value: &Value) -> Option<Self> {
        Some(Self {
            letter: unhex(value["letter"].as_str()?).ok()?,
            memo: value["memo"].as_str()?.to_string(),
            outpoint: value["outpoint"].as_str()?.parse().ok()?,
            received: u32::try_from(value["received"].as_u64()?).ok()?,
            reply_mail: key_bytes(&unhex(value["replyMail"].as_str()?).ok()?).ok()?,
            reply_url: value["replyUrl"].as_str()?.to_string(),
            ruling: match value["ruling"].as_str() {
                Some(text) => Some(unhex(text).ok()?),
                None => None,
            },
            citation: citation_from(value),
            public_ruling: match value["publicRuling"].as_str() {
                Some(text) => Some(unhex(text).ok()?),
                None => None,
            },
        })
    }
}

/// Cases by escrow and the branch the letter asks for.
#[derive(Default)]
pub(crate) struct ModeratorDesk {
    cases: BTreeMap<(EscrowId, u8), Case>,
}

impl ModeratorDesk {
    pub(crate) fn load(store: &EscrowStore) -> Self {
        let mut cases = BTreeMap::new();
        for (name, record) in store.list(Role::Moderator) {
            let parsed = name.split_once('-').and_then(|(id, party)| Some((parse_id(id)?, branch_byte(party)?)));
            match (parsed, Case::from_record(&record)) {
                (Some(key), Some(case)) => {
                    cases.insert(key, case);
                }
                _ => eprintln!("ob-node: moderator case {name} could not be read"),
            }
        }
        Self { cases }
    }
}

fn branch_byte(name: &str) -> Option<u8> {
    match name {
        "buyer" => Some(Party::Buyer as u8),
        "vendor" => Some(Party::Vendor as u8),
        _ => None,
    }
}

fn party_of(byte: u8) -> Party {
    if byte == Party::Vendor as u8 {
        Party::Vendor
    } else {
        Party::Buyer
    }
}

fn party_name(party: Party) -> &'static str {
    match party {
        Party::Buyer => "buyer",
        Party::Vendor => "vendor",
    }
}

fn case_name(id: &EscrowId, party: Party) -> String {
    format!("{}-{}", id_hex(id), party_name(party))
}

/// This node's chain source, followed by script. Confirmed outputs only.
pub(crate) struct ChainView<'a>(pub &'a dyn ChainSource);

impl FundingChain for ChainView<'_> {
    fn funding_output(&self, outpoint: &OutPoint, script: &Script, not_before: u32) -> Result<Option<TxOut>, EscrowError> {
        let txs = self.0.script_txs(script, not_before).map_err(|err| {
            eprintln!("ob-node: moderator chain view: {err}");
            EscrowError::ChainNotReady
        })?;
        Ok(txs
            .iter()
            .filter(|tx| tx.txid == outpoint.txid && tx.height.is_some())
            .flat_map(|tx| tx.outputs.iter())
            .find(|(vout, _)| *vout == outpoint.vout)
            .map(|(_, sats)| TxOut {
                value: bitcoin::Amount::from_sat(*sats),
                script_pubkey: script.to_owned(),
            }))
    }
}

fn letter_digest(letter: &[u8]) -> [u8; 32] {
    sha256::Hash::hash(letter).to_byte_array()
}

impl Node {
    fn moderator_desk(&self) -> MutexGuard<'_, ModeratorDesk> {
        self.escrow
            .moderator
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn moderator(&self) -> Result<Moderator<ChainView<'_>, &FileRulingStore>, String> {
        Moderator::new(
            moderator_keys(self.network)?,
            moderator_record(self.network)?,
            ChainView(&*self.chain),
            &self.escrow.rulings,
        )
        .map(|moderator| moderator.with_cache(self.escrow.letters.clone()))
        .map_err(|err| err.to_string())
    }

    pub(crate) fn moderator_mail(&self, mail: Mail) -> Result<(), String> {
        let writer = match mail.kind {
            Kind::Dispute => Party::Buyer,
            Kind::VendorDispute => Party::Vendor,
            other => return Err(format!("a moderator does not take {other:?}")),
        };
        self.moderator_review(&mail, writer)
    }

    /// Review a letter from `writer`, who must be the party it favors.
    fn moderator_review(&self, mail: &Mail, writer: Party) -> Result<(), String> {
        let fields = mail.as_fields();
        let citation = match fields.len() {
            3 => None,
            6 => Some(citation_from_fields(fields)?),
            _ => return Err("a Dispute message carries 3 fields".into()),
        };
        let reply_mail = key_bytes(&fields[1])?;
        let reply_url = crate::peer_http::sale_base(&String::from_utf8(fields[2].clone()).map_err(|err| err.to_string())?)?;
        let received = self.chain.tip_height()?;
        let digest = letter_digest(&fields[0]);
        let limits: &DisputeLimits = &self.escrow.limits;
        let mut gate = |key: &openbazaar_escrow::RulingKey, _: Party| limits.admit(digest, key, now_secs());
        let case = match self.moderator()?.review_gated(&fields[0], received, &mut gate) {
            Ok(case) => case,
            Err(EscrowError::ChainNotReady) => return Err(format!("{RETRY_LATER}the funding is not checked yet")),
            Err(err) => return Err(err.to_string()),
        };
        if case.favored() != writer {
            return Err("a dispute letter must ask for its writer's branch".into());
        }
        if let Some(citation) = citation {
            check_citation(&mail.escrow, case.terms(), &citation)?;
        }
        let key = (mail.escrow, case.favored() as u8);
        let mut desk = self.moderator_desk();
        if desk.cases.contains_key(&key) {
            return Ok(());
        }
        let case = Case {
            letter: fields[0].clone(),
            memo: String::from_utf8_lossy(case.memo()).into_owned(),
            outpoint: case.funding_outpoint(),
            received,
            reply_mail,
            reply_url,
            ruling: None,
            citation,
            public_ruling: None,
        };
        self.escrow
            .store
            .save(Role::Moderator, &case_name(&mail.escrow, party_of(key.1)), &case.to_record())?;
        desk.cases.insert(key, case);
        Ok(())
    }

    pub(crate) fn disputes_value(&self) -> Value {
        let desk = self.moderator_desk();
        let cases: Vec<Value> = desk
            .cases
            .iter()
            .map(|((id, branch), case)| {
                let favored = party_of(*branch);
                json!({
                    "id": id_hex(id),
                    "favors": party_name(favored),
                    "memo": case.memo,
                    "outpoint": case.outpoint.to_string(),
                    "receivedHeight": case.received,
                    "ruled": case.ruling.is_some(),
                })
            })
            .collect();
        json!({ "disputes": cases })
    }

    /// Rule on a reviewed letter and send the ruling to the party that wrote it.
    pub fn rule_dispute(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        let favor = match body.as_object() {
            Some(map) if map.len() == 1 => map.get("favor").and_then(Value::as_str),
            _ => None,
        };
        let favor = match favor {
            Some("buyer") => Party::Buyer,
            Some("vendor") => Party::Vendor,
            _ => {
                return Err(PayError::Invalid(
                    "Send {\"favor\": \"buyer\"} or {\"favor\": \"vendor\"}.".into(),
                ))
            }
        };
        let escrow_id = parse_id(id).ok_or(PayError::NotFound)?;
        let lock = self.escrow.locks.get(&escrow_id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let (letter, received) = {
            let desk = self.moderator_desk();
            let Some(case) = desk.cases.get(&(escrow_id, favor as u8)) else {
                let other = match favor {
                    Party::Buyer => Party::Vendor,
                    Party::Vendor => Party::Buyer,
                };
                if desk.cases.contains_key(&(escrow_id, other as u8)) {
                    let asks = party_name(other);
                    return Err(PayError::Invalid(format!(
                        "This letter asks for the {asks}. Only the {asks} branch can be opened from it."
                    )));
                }
                return Err(PayError::NotFound);
            };
            if case.ruling.is_some() {
                return Err(PayError::Conflict("This dispute has a ruling.".into()));
            }
            (case.letter.clone(), case.received)
        };
        let ruling = self
            .moderator()
            .map_err(PayError::Failed)?
            .open(&letter, received)
            .map_err(|err| PayError::Conflict(err.to_string()))?;
        let (url, to) = self.store_ruling(&escrow_id, favor, &ruling).map_err(PayError::Failed)?;
        self.send_mail(&url, &to, &Mail::new(Kind::Ruling, escrow_id, vec![ruling.to_bytes()]))
            .map_err(|err| PayError::Chain(format!("The party's node refused the ruling: {err}")))?;
        Ok(json!({ "id": id, "favors": party_name(favor), "ruled": true }))
    }

    fn store_ruling(&self, id: &EscrowId, favor: Party, ruling: &ModeratorRuling) -> Result<(String, StoreKey), String> {
        let mut desk = self.moderator_desk();
        let case = desk.cases.get_mut(&(*id, favor as u8)).ok_or("the case is gone")?;
        case.ruling = Some(ruling.to_bytes());
        if let Some(citation) = case.citation {
            let keys = moderator_keys(self.network)?;
            case.public_ruling = Some(
                keys.sign_public_ruling(&ruling.to_bytes(), citation.store_key, citation.listing_id)
                    .map_err(|err| err.to_string())?,
            );
        }
        self.escrow.store.save(Role::Moderator, &case_name(id, favor), &case.to_record())?;
        Ok((case.reply_url.clone(), case.reply_mail))
    }
}

fn fixed<const N: usize>(text: &str) -> Option<[u8; N]> {
    unhex(text).ok().and_then(|bytes| bytes.as_slice().try_into().ok())
}

fn citation_from(value: &Value) -> Option<Citation> {
    Some(Citation {
        listing_id: fixed(value["listingId"].as_str()?)?,
        store_key: fixed(value["storeKey"].as_str()?)?,
        store_signature: fixed(value["storeSignature"].as_str()?)?,
    })
}

fn citation_from_fields(fields: &[Vec<u8>]) -> Result<Citation, String> {
    Ok(Citation {
        listing_id: fields[3].as_slice().try_into().map_err(|_| "bad listing id")?,
        store_key: fields[4].as_slice().try_into().map_err(|_| "bad store key")?,
        store_signature: fields[5].as_slice().try_into().map_err(|_| "bad store signature")?,
    })
}

fn check_citation(id: &EscrowId, terms: &openbazaar_escrow::EscrowTerms, citation: &Citation) -> Result<(), String> {
    let key = XOnlyPublicKey::from_slice(&citation.store_key).map_err(|err| err.to_string())?;
    let signature = schnorr::Signature::from_slice(&citation.store_signature).map_err(|err| err.to_string())?;
    Secp256k1::new()
        .verify_schnorr(&signature, &offer_digest_with_listing(id, terms, &citation.listing_id), &key)
        .map_err(|_| "the store signature does not cover this listing".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::MockChain;

    #[test]
    fn the_chain_view_finds_funding_by_script_and_waits_while_filters_scan() {
        let chain = MockChain::new();
        chain.0.filters.store(true, std::sync::atomic::Ordering::Relaxed);
        let script = bitcoin::ScriptBuf::new_p2a();
        let address = bitcoin::Address::from_script(&script, bitcoin::Network::Testnet4).unwrap();
        let txid = chain.pay_external(&address.to_string(), 5_000);
        let outpoint = OutPoint { txid, vout: 0 };
        let view = ChainView(&chain);
        assert!(chain.output(&outpoint).is_err(), "a filter backend cannot look the txid up");
        assert_eq!(view.funding_output(&outpoint, &script, 7), Err(EscrowError::ChainNotReady));
        assert_eq!(view.funding_output(&outpoint, &script, 7), Ok(None), "unconfirmed");
        chain.mine();
        let found = view.funding_output(&outpoint, &script, 7).unwrap().unwrap();
        assert_eq!(found.value.to_sat(), 5_000);
        assert_eq!(view.funding_output(&OutPoint { txid, vout: 1 }, &script, 7), Ok(None));
        assert_eq!(*chain.0.followed.lock().unwrap(), vec![(script, 7)]);
    }
}
