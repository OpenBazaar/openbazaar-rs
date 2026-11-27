//! Local WTB / WTS board and sealed contact threads.
//!
//! Posts are [`SignedClassified`] records kept in `classifieds.obv`. Contact
//! mail is a sealed message (`kind: "contact"`) to the post's mailbox key —
//! separate from order chat. Threads pin the peer's sender key and reply
//! route the same way bids do.
//!
//! **Privacy at rest (audit #252):** Contact threads (including plaintext
//! message bodies) are stored inside the vault-sealed `classifieds.obv`.
//! Vault compromise or `--vault file` exposure reveals conversation content.
//! Per-thread encryption is a future improvement; thread expiry is supported
//! through classified expiration (expired classifieds' threads can be purged).

use std::path::{Path, PathBuf};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use openbazaar_protocol::{
    add_days, Classified, ClassifiedCategory, ClassifiedKind, SignedClassified,
    DEFAULT_LIFE_DAYS, MAX_LIFE_DAYS,
};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};
use tormail::{seal_message, ReplyPublic, SenderKey, StoreKey};

use crate::escrow_mail::{hex, unhex};
use crate::messages::{self, MAX_MESSAGES};
use crate::order_mail::{body_bytes, mail_key, MailError, STORE_MAIL};
use crate::own_listings::{capitalize, StoreError};
use crate::sales::{refused, retry};
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "classifieds.obv";
const MAX_POSTS: usize = 2_000;
const MAX_THREADS: usize = 500;
const MAX_THREADS_PER_PEER: usize = 10;
const MAX_OWN_LIVE: usize = 50;

#[derive(Clone)]
struct Thread {
    id: [u8; 16],
    classified_id: [u8; 16],
    poster: [u8; 32],
    peer: [u8; 32],
    /// Whether this node is the classified poster (receiving contact).
    as_poster: bool,
    reply_url: String,
    reply_key: [u8; 32],
    drop: [u8; 16],
    title: String,
    messages: Value,
}

pub(crate) struct ClassifiedBoard {
    path: PathBuf,
    vault: Vault,
    pub(crate) posts: Vec<SignedClassified>,
    threads: Vec<Thread>,
}

impl ClassifiedBoard {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut board = Self { path, vault, posts: Vec::new(), threads: Vec::new() };
        let Some(bytes) = board.vault.read_sealed(&board.path)? else {
            return Ok(board);
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
        for item in value["posts"].as_array().into_iter().flatten() {
            if let Some(post) = item.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| SignedClassified::decode(&bytes).ok()) {
                if post.verify().is_ok() {
                    board.posts.push(post);
                }
            }
        }
        for row in value["threads"].as_array().into_iter().flatten() {
            if let Some(thread) = thread_of(row) {
                board.threads.push(thread);
            }
        }
        Ok(board)
    }

    fn save(&self) -> Result<(), StoreError> {
        let posts: Vec<String> = self.posts.iter().filter_map(|post| post.encode().ok().map(|bytes| hex(&bytes))).collect();
        let threads: Vec<Value> = self.threads.iter().map(thread_value).collect();
        let bytes = serde_json::to_vec(&json!({ "v": 1, "posts": posts, "threads": threads })).map_err(|err| StoreError::Failed(err.to_string()))?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| StoreError::Failed(err.to_string()))
    }

    fn take(&mut self, post: SignedClassified) -> bool {
        if post.verify().is_err() {
            return false;
        }
        if let Some(old) = self.posts.iter_mut().find(|old| old.terms.id == post.terms.id) {
            if old.terms.poster != post.terms.poster {
                return false;
            }
            *old = post;
            return true;
        }
        if self.posts.len() >= MAX_POSTS {
            return false;
        }
        self.posts.push(post);
        true
    }
}

fn thread_of(value: &Value) -> Option<Thread> {
    Some(Thread {
        id: crate::escrow_mail::parse_id(value["id"].as_str()?)?,
        classified_id: crate::escrow_mail::parse_id(value["classifiedId"].as_str()?)?,
        poster: crate::stores::unhex32(value["poster"].as_str()?)?,
        peer: crate::stores::unhex32(value["peer"].as_str()?)?,
        as_poster: value["asPoster"].as_bool()?,
        reply_url: value["replyUrl"].as_str()?.to_string(),
        reply_key: crate::stores::unhex32(value["replyKey"].as_str()?)?,
        drop: crate::escrow_mail::parse_id(value["drop"].as_str()?)?,
        title: value["title"].as_str().unwrap_or("").to_string(),
        messages: value["messages"].clone(),
    })
}

fn thread_value(thread: &Thread) -> Value {
    json!({
        "id": hex(&thread.id),
        "classifiedId": hex(&thread.classified_id),
        "poster": hex(&thread.poster),
        "peer": hex(&thread.peer),
        "asPoster": thread.as_poster,
        "replyUrl": thread.reply_url,
        "replyKey": hex(&thread.reply_key),
        "drop": hex(&thread.drop),
        "title": thread.title,
        "messages": thread.messages,
    })
}

fn thread_id(classified_id: &[u8; 16], poster: &[u8; 32], peer: &[u8; 32]) -> [u8; 16] {
    let mut engine = sha256::Hash::engine();
    engine.input(b"ob/contact/v1");
    engine.input(classified_id);
    engine.input(poster);
    engine.input(peer);
    let digest = sha256::Hash::from_engine(engine).to_byte_array();
    digest[..16].try_into().expect("16 bytes")
}

fn post_value(post: &SignedClassified, today: u32) -> Value {
    let life = post.terms.life_days().unwrap_or(0);
    json!({
        "id": hex(&post.terms.id),
        "kind": post.terms.kind.as_str(),
        "category": post.terms.category.as_str(),
        "title": post.terms.title,
        "body": post.terms.body,
        "region": post.terms.region,
        "priceHint": post.terms.price_hint,
        "currency": post.terms.currency,
        "createdOn": post.terms.created_on,
        "expiresOn": post.terms.expires_on,
        "lifeDays": life,
        "expired": post.terms.expired_on(today),
        "poster": hex(&post.terms.poster),
        "mailboxKey": hex(&post.terms.mailbox_key),
        "onion": post.terms.onion,
        "record": hex(&post.encode().unwrap_or_default()),
    })
}

/// Read-only post for the public classifieds board: no mailbox key, onion, or record.
fn public_post_value(post: &SignedClassified, today: u32) -> Value {
    let life = post.terms.life_days().unwrap_or(0);
    json!({
        "id": hex(&post.terms.id),
        "kind": post.terms.kind.as_str(),
        "category": post.terms.category.as_str(),
        "title": post.terms.title,
        "body": post.terms.body,
        "region": post.terms.region,
        "priceHint": post.terms.price_hint,
        "currency": post.terms.currency,
        "createdOn": post.terms.created_on,
        "expiresOn": post.terms.expires_on,
        "lifeDays": life,
        "expired": post.terms.expired_on(today),
        "poster": hex(&post.terms.poster),
    })
}

fn public_thread(thread: &Thread) -> Value {
    json!({
        "id": hex(&thread.id),
        "classifiedId": hex(&thread.classified_id),
        "poster": hex(&thread.poster),
        "peer": hex(&thread.peer),
        "asPoster": thread.as_poster,
        "title": thread.title,
        "messages": thread.messages,
    })
}

impl Node {
    pub(crate) fn classifieds(&self) -> std::sync::MutexGuard<'_, ClassifiedBoard> {
        self.classifieds.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// `GET /classifieds`: live posts, newest first.
    /// Optional `kind` / `category` / `q` / `region`, plus `limit` (default 50, max 100) and `offset`.
    pub(crate) fn classifieds_value(
        &self,
        kind: Option<&str>,
        category: Option<&str>,
        q: Option<&str>,
        region: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Value {
        let today = crate::own_store::today_yyyymmdd();
        let kind = kind.and_then(|name| ClassifiedKind::parse(name).ok());
        let category = category.and_then(|name| ClassifiedCategory::parse(name).ok());
        let needle = q.map(str::trim).filter(|text| !text.is_empty()).map(|text| text.to_ascii_lowercase());
        let region_needle = region.map(str::trim).filter(|text| !text.is_empty()).map(|text| text.to_ascii_lowercase());
        let limit = limit.unwrap_or(50).clamp(1, 100);
        let offset = offset.unwrap_or(0);
        let board = self.classifieds();
        let mut posts: Vec<&SignedClassified> = board
            .posts
            .iter()
            .filter(|post| !post.terms.expired_on(today))
            .filter(|post| kind.is_none_or(|want| post.terms.kind == want))
            .filter(|post| category.is_none_or(|want| post.terms.category == want))
            .filter(|post| {
                region_needle.as_ref().is_none_or(|want| post.terms.region.to_ascii_lowercase().contains(want))
            })
            .filter(|post| {
                needle.as_ref().is_none_or(|want| {
                    post.terms.title.to_ascii_lowercase().contains(want)
                        || post.terms.body.to_ascii_lowercase().contains(want)
                        || post.terms.region.to_ascii_lowercase().contains(want)
                })
            })
            .collect();
        posts.sort_by(|a, b| {
            b.terms
                .created_on
                .cmp(&a.terms.created_on)
                .then_with(|| b.terms.id.cmp(&a.terms.id))
        });
        let total = posts.len();
        let items: Vec<Value> = posts.into_iter().skip(offset).take(limit).map(|post| post_value(post, today)).collect();
        json!({ "classifieds": items, "today": today, "total": total, "limit": limit, "offset": offset })
    }

    /// `GET /public/classifieds-board`: read-only board for guest browsers.
    /// Same filtering and pagination as `classifieds_value`, but strips private
    /// fields (mailboxKey, onion, record) from every post.
    pub(crate) fn public_classifieds_value(
        &self,
        kind: Option<&str>,
        category: Option<&str>,
        q: Option<&str>,
        region: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Value {
        let today = crate::own_store::today_yyyymmdd();
        let kind = kind.and_then(|name| ClassifiedKind::parse(name).ok());
        let category = category.and_then(|name| ClassifiedCategory::parse(name).ok());
        let needle = q.map(str::trim).filter(|text| !text.is_empty()).map(|text| text.to_ascii_lowercase());
        let region_needle = region.map(str::trim).filter(|text| !text.is_empty()).map(|text| text.to_ascii_lowercase());
        let limit = limit.unwrap_or(50).clamp(1, 100);
        let offset = offset.unwrap_or(0);
        let board = self.classifieds();
        let mut posts: Vec<&SignedClassified> = board
            .posts
            .iter()
            .filter(|post| !post.terms.expired_on(today))
            .filter(|post| kind.is_none_or(|want| post.terms.kind == want))
            .filter(|post| category.is_none_or(|want| post.terms.category == want))
            .filter(|post| {
                region_needle.as_ref().is_none_or(|want| post.terms.region.to_ascii_lowercase().contains(want))
            })
            .filter(|post| {
                needle.as_ref().is_none_or(|want| {
                    post.terms.title.to_ascii_lowercase().contains(want)
                        || post.terms.body.to_ascii_lowercase().contains(want)
                        || post.terms.region.to_ascii_lowercase().contains(want)
                })
            })
            .collect();
        posts.sort_by(|a, b| {
            b.terms
                .created_on
                .cmp(&a.terms.created_on)
                .then_with(|| b.terms.id.cmp(&a.terms.id))
        });
        let total = posts.len();
        let items: Vec<Value> = posts.into_iter().skip(offset).take(limit).map(|post| public_post_value(post, today)).collect();
        json!({ "classifieds": items, "today": today, "total": total, "limit": limit, "offset": offset })
    }

    pub(crate) fn classified_value(&self, id: &str) -> Option<Value> {
        let id = crate::escrow_mail::parse_id(id)?;
        let today = crate::own_store::today_yyyymmdd();
        let board = self.classifieds();
        board.posts.iter().find(|post| post.terms.id == id).map(|post| post_value(post, today))
    }

    /// `POST /classifieds`: sign and keep one post from this node's store key.
    pub(crate) fn create_classified(&self, body: &Value) -> Result<Value, StoreError> {
        let secret = self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let poster = openbazaar_protocol::xonly(&secret);
        let mailbox = {
            let wallet = self.buyer_wallet();
            let key = wallet.watch.store_mailbox_key().map_err(|err| self.classified_letter_err(err))?;
            let secret = tormail::StoreSecret::from_bytes(*key).map_err(|err| StoreError::Failed(err.to_string()))?;
            *secret.public_key().as_bytes()
        };
        let kind = ClassifiedKind::parse(body["kind"].as_str().unwrap_or("")).map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
        let category = ClassifiedCategory::parse(body["category"].as_str().unwrap_or("")).map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
        let title = body["title"].as_str().unwrap_or("").trim();
        let text = body["body"].as_str().unwrap_or("").trim();
        let region = body["region"].as_str().unwrap_or("").trim();
        let life = body["lifeDays"].as_u64().unwrap_or(u64::from(DEFAULT_LIFE_DAYS));
        if !(1..=u64::from(MAX_LIFE_DAYS)).contains(&life) {
            return Err(StoreError::Invalid(format!("A classified lives between 1 and {MAX_LIFE_DAYS} days.")));
        }
        let life = life as u32;
        let created_on = crate::own_store::today_yyyymmdd();
        let expires_on = add_days(created_on, life).ok_or_else(|| StoreError::Invalid("That expiry could not be set.".into()))?;
        let price_hint = body["priceHint"].as_u64().unwrap_or(0);
        let currency = body["currency"].as_str().unwrap_or("").trim().to_uppercase();
        let onion = self
            .advertised_onion
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .unwrap_or_default();
        let mut id = [0u8; 16];
        StdRng::from_os_rng().fill_bytes(&mut id);
        if id == [0; 16] {
            id[0] = 1;
        }
        let post = Classified {
            id,
            kind,
            category,
            title: title.to_string(),
            body: text.to_string(),
            region: region.to_string(),
            price_hint,
            currency: if price_hint > 0 { currency } else { String::new() },
            created_on,
            expires_on,
            poster,
            mailbox_key: mailbox,
            onion,
        };
        let signed = SignedClassified::sign(post, &secret).map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
        let today = created_on;
        let mut board = self.classifieds();
        let live_own = board.posts.iter().filter(|post| post.terms.poster == poster && !post.terms.expired_on(today)).count();
        if live_own >= MAX_OWN_LIVE {
            return Err(StoreError::Invalid(format!("Keep at most {MAX_OWN_LIVE} live classifieds.")));
        }
        if !board.take(signed.clone()) {
            return Err(StoreError::Failed("That classified could not be kept.".into()));
        }
        if let Err(err) = board.save() {
            board.posts.retain(|post| post.terms.id != id);
            return Err(err);
        }
        Ok(post_value(&signed, today))
    }

    /// Keep a verified post from another node (or a redelivery of our own).
    pub(crate) fn accept_classified(&self, bytes: &[u8]) -> Result<bool, String> {
        let signed = SignedClassified::decode(bytes).map_err(|err| err.to_string())?;
        signed.verify().map_err(|err| err.to_string())?;
        let mut board = self.classifieds();
        let changed = board.take(signed);
        if changed {
            board.save().map_err(|err| match err {
                StoreError::Failed(message) | StoreError::Invalid(message) => message,
                StoreError::NotFound => "not found".into(),
                StoreError::Locked(state) => state.into(),
            })?;
        }
        Ok(changed)
    }

    pub(crate) fn delete_classified(&self, id: &str) -> Result<(), StoreError> {
        let id = crate::escrow_mail::parse_id(id).ok_or(StoreError::NotFound)?;
        let poster = self.own_signing_key().map(|secret| openbazaar_protocol::xonly(&secret));
        let mut board = self.classifieds();
        let Some(index) = board.posts.iter().position(|post| post.terms.id == id) else {
            return Err(StoreError::NotFound);
        };
        if poster.is_some_and(|key| board.posts[index].terms.poster != key) {
            return Err(StoreError::Invalid("That classified is not yours.".into()));
        }
        let removed = board.posts.remove(index);
        if let Err(err) = board.save() {
            board.posts.insert(index, removed);
            return Err(err);
        }
        Ok(())
    }

    /// Delete a contact thread and its messages from the vault-sealed store.
    pub(crate) fn delete_contact_thread(&self, id: &str) -> Result<(), StoreError> {
        let id = crate::escrow_mail::parse_id(id).ok_or(StoreError::NotFound)?;
        let mut board = self.classifieds();
        let Some(index) = board.threads.iter().position(|thread| thread.id == id) else {
            return Err(StoreError::NotFound);
        };
        let removed = board.threads.remove(index);
        if let Err(err) = board.save() {
            board.threads.insert(index, removed);
            return Err(err);
        }
        Ok(())
    }

    pub(crate) fn contact_threads_value(&self) -> Value {
        let threads: Vec<Value> = self.classifieds().threads.iter().map(public_thread).collect();
        json!({ "threads": threads })
    }

    pub(crate) fn contact_thread_value(&self, id: &str) -> Option<Value> {
        let id = crate::escrow_mail::parse_id(id)?;
        self.classifieds().threads.iter().find(|thread| thread.id == id).map(public_thread)
    }

    /// `POST /classifieds/{id}/contact`: open or continue a contact thread.
    pub(crate) fn contact_classified(&self, id: &str, body: &Value) -> Result<Value, StoreError> {
        let text = messages::clean_text(body).map_err(StoreError::Invalid)?;
        let classified_id = crate::escrow_mail::parse_id(id).ok_or(StoreError::NotFound)?;
        let post = self
            .classifieds()
            .posts
            .iter()
            .find(|post| post.terms.id == classified_id)
            .cloned()
            .ok_or(StoreError::NotFound)?;
        let today = crate::own_store::today_yyyymmdd();
        if post.terms.expired_on(today) {
            return Err(StoreError::Invalid("That classified has expired.".into()));
        }
        if post.terms.poster == *self.own_store.as_bytes() {
            return Err(StoreError::Invalid("That is your own classified.".into()));
        }
        let vendor = StoreKey::from_bytes(post.terms.mailbox_key).map_err(|err| StoreError::Failed(err.to_string()))?;
        let conversation = [classified_id.as_slice(), post.terms.poster.as_slice()].concat();
        let from = SenderKey::derive(&*self.letter_root_for_contact()?, "contact", &conversation);
        let (reply, drop) = self.contact_reply_secret(&classified_id, &post.terms.poster)?;
        let reply_url = crate::letter_auth::choose_reply(None, self.sale_reply_url().as_deref())
            .ok_or_else(|| StoreError::Invalid("This node has no address for a reply.".into()))?;
        let message_id = messages::message_id();
        let now = crate::escrow_outbox::now_secs();
        let peer = from.id().0;
        let thread_id = thread_id(&classified_id, &post.terms.poster, &peer);
        {
            let mut board = self.classifieds();
            if board.threads.iter().all(|thread| thread.id != thread_id) {
                if board.threads.len() >= MAX_THREADS {
                    return Err(StoreError::Invalid("Too many contact threads.".into()));
                }
                board.threads.push(Thread {
                    id: thread_id,
                    classified_id,
                    poster: post.terms.poster,
                    peer,
                    as_poster: false,
                    reply_url: reply_url.clone(),
                    reply_key: *reply.public_key().as_bytes(),
                    drop: drop.0,
                    title: post.terms.title.clone(),
                    messages: json!([]),
                });
            }
            let Some(thread) = board.threads.iter_mut().find(|thread| thread.id == thread_id) else {
                return Err(StoreError::Failed("The contact thread could not be kept.".into()));
            };
            if !thread.messages.is_array() {
                thread.messages = json!([]);
            }
            let list = thread.messages.as_array_mut().expect("array");
            if !list.iter().any(|message| message["id"] == message_id) {
                list.push(json!({ "id": message_id, "from": "me", "text": text, "at": now }));
                if list.len() > MAX_MESSAGES {
                    list.drain(..list.len() - MAX_MESSAGES);
                }
            }
            thread.reply_url = reply_url.clone();
            thread.reply_key = *reply.public_key().as_bytes();
            thread.drop = drop.0;
            board.save()?;
        }
        let letter_body = json!({
            "v": 1,
            "kind": "contact",
            "classified": hex(&classified_id),
            "id": message_id,
            "text": text,
            "replyUrl": reply_url,
            "replyKey": hex(reply.public_key().as_bytes()),
            "drop": hex(&drop.0),
            "title": post.terms.title,
        });
        let bytes = body_bytes(&letter_body);
        let letter = seal_message(&bytes, &vendor, &from).map_err(|err| StoreError::Failed(err.to_string()))?;
        let target = self.contact_mail_url(&post)?;
        let posted = letter.to_bytes();
        match self.order_outbox.send(mail_key(&target, &conversation, &bytes), &target, &posted) {
            Ok(crate::escrow_outbox::Sent::Delivered | crate::escrow_outbox::Sent::Queued(_)) => {}
            Err(err) => return Err(StoreError::Invalid(format!("The poster did not take that message. {err}"))),
        }
        self.contact_thread_value(&hex(&thread_id)).ok_or_else(|| StoreError::Failed("The contact thread could not be read.".into()))
    }

    /// `POST /contact/{id}/message`: continue an existing thread.
    pub(crate) fn contact_reply(&self, id: &str, body: &Value) -> Result<Value, StoreError> {
        let text = messages::clean_text(body).map_err(StoreError::Invalid)?;
        let id = crate::escrow_mail::parse_id(id).ok_or(StoreError::NotFound)?;
        let message_id = messages::message_id();
        let now = crate::escrow_outbox::now_secs();
        let (as_poster, reply_url, reply_key, drop, classified_id, poster, peer, title) = {
            let mut board = self.classifieds();
            let Some(thread) = board.threads.iter_mut().find(|thread| thread.id == id) else {
                return Err(StoreError::NotFound);
            };
            if !thread.messages.is_array() {
                thread.messages = json!([]);
            }
            let list = thread.messages.as_array_mut().expect("array");
            list.push(json!({ "id": message_id, "from": "me", "text": text, "at": now }));
            if list.len() > MAX_MESSAGES {
                list.drain(..list.len() - MAX_MESSAGES);
            }
            let snapshot = (
                thread.as_poster,
                thread.reply_url.clone(),
                thread.reply_key,
                thread.drop,
                thread.classified_id,
                thread.poster,
                thread.peer,
                thread.title.clone(),
            );
            board.save()?;
            snapshot
        };
        if as_poster {
            self.queue_buyer_reply(&reply_url, reply_key, drop, &json!({
                "v": 1,
                "kind": "contact",
                "classified": hex(&classified_id),
                "id": message_id,
                "text": text,
                "title": title,
            }))
            .map_err(StoreError::Failed)?;
        } else {
            let post = self
                .classifieds()
                .posts
                .iter()
                .find(|post| post.terms.id == classified_id)
                .cloned()
                .ok_or_else(|| StoreError::Invalid("That classified is no longer on this node.".into()))?;
            let vendor = StoreKey::from_bytes(post.terms.mailbox_key).map_err(|err| StoreError::Failed(err.to_string()))?;
            let conversation = [classified_id.as_slice(), poster.as_slice()].concat();
            let from = SenderKey::derive(&*self.letter_root_for_contact()?, "contact", &conversation);
            let (reply, drop_id) = self.contact_reply_secret(&classified_id, &poster)?;
            let reply_url = crate::letter_auth::choose_reply(None, self.sale_reply_url().as_deref())
                .ok_or_else(|| StoreError::Invalid("This node has no address for a reply.".into()))?;
            let letter_body = json!({
                "v": 1,
                "kind": "contact",
                "classified": hex(&classified_id),
                "id": message_id,
                "text": text,
                "replyUrl": reply_url,
                "replyKey": hex(reply.public_key().as_bytes()),
                "drop": hex(&drop_id.0),
                "title": title,
            });
            let bytes = body_bytes(&letter_body);
            let letter = seal_message(&bytes, &vendor, &from).map_err(|err| StoreError::Failed(err.to_string()))?;
            let target = self.contact_mail_url(&post)?;
            match self.order_outbox.send(mail_key(&target, &conversation, &bytes), &target, &letter.to_bytes()) {
                Ok(crate::escrow_outbox::Sent::Delivered | crate::escrow_outbox::Sent::Queued(_)) => {}
                Err(err) => return Err(StoreError::Invalid(format!("The poster did not take that message. {err}"))),
            }
            let _ = (peer, drop);
        }
        self.contact_thread_value(&hex(&id)).ok_or(StoreError::NotFound)
    }

    /// Where to post sealed contact for `post`: its onion, else a followed store URL.
    fn contact_mail_url(&self, post: &SignedClassified) -> Result<String, StoreError> {
        if let Some(url) = contact_target(&post.terms.onion, None) {
            return Ok(url);
        }
        let poster = openbazaar_protocol::StoreId::from_bytes(post.terms.poster);
        if let Some(base) = self.replicas().url_of(&poster) {
            return Ok(format!("{base}{STORE_MAIL}"));
        }
        Err(StoreError::Invalid(
            "Follow that store before contacting, or wait until its onion is on the post.".into(),
        ))
    }

    /// A sealed reply on a contact thread this node opened (`as_poster: false`).
    /// Returns `Ok(true)` when handled, `Ok(false)` when the drop is not ours.
    pub(crate) fn receive_contact_reply(&self, drop: &[u8; 16], sealed: &tormail::SealedReply) -> Result<bool, MailError> {
        let (classified_id, poster) = {
            let board = self.classifieds();
            let Some(thread) = board.threads.iter().find(|thread| thread.drop == *drop && !thread.as_poster) else {
                return Ok(false);
            };
            (thread.classified_id, thread.poster)
        };
        let (secret, _) = self.contact_reply_secret(&classified_id, &poster).map_err(|err| retry(store_detail(err)))?;
        let opened = tormail::open_reply(sealed, &secret).map_err(|_| refused("that reply does not open"))?;
        let body: Value = serde_json::from_slice(&opened.body).map_err(|_| refused("that reply could not be read"))?;
        if body["kind"] != "contact" {
            return Err(refused("that reply is not a contact message"));
        }
        let (message_id, text) = messages::received(&body).ok_or_else(|| refused("that contact could not be read"))?;
        let now = crate::escrow_outbox::now_secs();
        let mut board = self.classifieds();
        let Some(thread) = board.threads.iter_mut().find(|thread| thread.drop == *drop && !thread.as_poster) else {
            return Ok(false);
        };
        if !thread.messages.is_array() {
            thread.messages = json!([]);
        }
        let list = thread.messages.as_array_mut().expect("array");
        if !list.iter().any(|message| message["id"] == message_id) {
            list.push(json!({ "id": message_id, "from": "them", "text": text, "at": now }));
            if list.len() > MAX_MESSAGES {
                list.drain(..list.len() - MAX_MESSAGES);
            }
        }
        board.save().map_err(|err| retry(store_detail(err)))?;
        Ok(true)
    }

    /// A sealed contact letter for this store's mailbox.
    pub(crate) fn receive_contact(&self, peer: &[u8; 32], body: &Value) -> Result<(), MailError> {
        let classified_id = crate::escrow_mail::parse_id(body["classified"].as_str().unwrap_or("")).ok_or_else(|| refused("that contact names no classified"))?;
        let (message_id, text) = messages::received(body).ok_or_else(|| refused("that contact could not be read"))?;
        let (url, key, drop) = contact_reply_route(body)?;
        let title = body["title"].as_str().unwrap_or("").to_string();
        let poster = *self.own_store.as_bytes();
        let post = self.classifieds().posts.iter().find(|post| post.terms.id == classified_id).cloned();
        let title = if title.is_empty() {
            post.as_ref().map(|post| post.terms.title.clone()).unwrap_or_default()
        } else {
            title
        };
        if let Some(post) = &post {
            if post.terms.poster != poster {
                return Err(refused("that classified is not this store's"));
            }
        }
        let thread_id = thread_id(&classified_id, &poster, peer);
        let now = crate::escrow_outbox::now_secs();
        let mut board = self.classifieds();
        let thread = match board.threads.iter_mut().find(|thread| thread.id == thread_id) {
            Some(thread) => thread,
            None => {
                if board.threads.len() >= MAX_THREADS {
                    return Err(refused("that store cannot take another contact right now"));
                }
                let peer_threads = board.threads.iter().filter(|thread| thread.peer == *peer).count();
                if peer_threads >= MAX_THREADS_PER_PEER {
                    return Err(refused("that sender has too many open contact threads"));
                }
                board.threads.push(Thread {
                    id: thread_id,
                    classified_id,
                    poster,
                    peer: *peer,
                    as_poster: true,
                    reply_url: url.clone(),
                    reply_key: key,
                    drop,
                    title,
                    messages: json!([]),
                });
                board.threads.last_mut().expect("just pushed")
            }
        };
        if thread.peer != *peer {
            return Err(refused("that contact is not from this thread"));
        }
        thread.reply_url = url;
        thread.reply_key = key;
        thread.drop = drop;
        if !thread.messages.is_array() {
            thread.messages = json!([]);
        }
        let list = thread.messages.as_array_mut().expect("array");
        if !list.iter().any(|message| message["id"] == message_id) {
            list.push(json!({ "id": message_id, "from": "them", "text": text, "at": now }));
            if list.len() > MAX_MESSAGES {
                list.drain(..list.len() - MAX_MESSAGES);
            }
        }
        board.save().map_err(|err| retry(store_detail(err)))?;
        Ok(())
    }

    fn letter_root_for_contact(&self) -> Result<zeroize::Zeroizing<[u8; 32]>, StoreError> {
        let wallet = self.buyer_wallet();
        let root = wallet.watch.letter_root().map_err(|err| self.classified_letter_err(err))?;
        Ok(zeroize::Zeroizing::new(*root))
    }

    fn contact_reply_secret(&self, classified_id: &[u8; 16], poster: &[u8; 32]) -> Result<(tormail::ReplySecret, tormail::DropId), StoreError> {
        let conversation = [classified_id.as_slice(), poster.as_slice()].concat();
        let root = self.letter_root_for_contact()?;
        let secret = tormail::letter::derive_secret(&root, "contact-reply", &conversation, 0);
        let drop = tormail::letter::derive_secret(&root, "contact-drop", &conversation, 0);
        Ok((
            tormail::ReplySecret::from_bytes(secret).expect("any 32 bytes are an X25519 secret"),
            tormail::DropId(drop[..16].try_into().expect("16 bytes")),
        ))
    }

    fn classified_letter_err(&self, err: String) -> StoreError {
        match self.lock.state() {
            crate::wallet_lock::State::Unlocked => StoreError::Failed(err),
            state => StoreError::Locked(state.as_str()),
        }
    }
}

fn contact_reply_route(body: &Value) -> Result<(String, [u8; 32], [u8; 16]), MailError> {
    let url = crate::peer_http::sale_base(body["replyUrl"].as_str().unwrap_or("")).map_err(|_| refused("that contact has no reply route"))?;
    let key = crate::stores::unhex32(body["replyKey"].as_str().unwrap_or("")).ok_or_else(|| refused("that contact has no reply route"))?;
    let drop = crate::escrow_mail::parse_id(body["drop"].as_str().unwrap_or("")).ok_or_else(|| refused("that contact has no reply route"))?;
    ReplyPublic::from_bytes(key).map_err(|_| refused("that contact has no reply route"))?;
    Ok((url, key, drop))
}

fn contact_target(onion: &str, fallback: Option<&str>) -> Option<String> {
    let onion = onion.trim();
    if !onion.is_empty() {
        let host = if onion.ends_with(".onion") { onion.to_string() } else { format!("{onion}.onion") };
        return Some(format!("http://{host}{STORE_MAIL}"));
    }
    fallback.map(|base| format!("{base}{STORE_MAIL}"))
}

fn store_detail(err: StoreError) -> String {
    match err {
        StoreError::Invalid(message) | StoreError::Failed(message) => message,
        StoreError::NotFound => "not found".into(),
        StoreError::Locked(state) => state.into(),
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use openbazaar_protocol::day_from_unix;

    #[test]
    fn thread_ids_are_stable_for_the_same_parties() {
        let classified = [1u8; 16];
        let poster = [2u8; 32];
        let peer = [3u8; 32];
        assert_eq!(thread_id(&classified, &poster, &peer), thread_id(&classified, &poster, &peer));
        assert_ne!(thread_id(&classified, &poster, &peer), thread_id(&classified, &poster, &[4u8; 32]));
    }

    #[test]
    fn day_helpers_match_protocol() {
        assert_eq!(day_from_unix(0), 19700101);
        assert_eq!(add_days(20261125, 7), Some(20261202));
    }
}
