//! Buyer–seller messages inside an order.
//!
//! A message rides the order mail that already exists: sealed to the other
//! party, signed by the sender key each side pinned for that order, queued
//! in the persistent outbox when the other node is offline. There is no
//! server, no new key, and no address to learn: you can only write to the
//! buyer or seller of an order you are part of.
//!
//! Text is plain: capped, control characters stripped, rendered as text by
//! the clients (never HTML), and no link is ever fetched. Messages are kept
//! with the order, sealed at rest with the rest of the sale/order record.
//! The receiver stamps arrival with its own clock, so a sender cannot move
//! a message in the thread by lying about the time.

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};

use crate::Node;

/// The longest message, in characters.
pub(crate) const MAX_TEXT: usize = 2_000;
/// How many messages one order keeps; the oldest fall off.
pub(crate) const MAX_MESSAGES: usize = 500;

/// A fresh message id: how both sides recognize a redelivered letter.
pub(crate) fn message_id() -> String {
    let mut bytes = [0u8; 16];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    crate::escrow_mail::hex(&bytes)
}

/// The text of a message to send, or why it cannot be sent.
pub(crate) fn clean_text(body: &Value) -> Result<String, String> {
    let raw = body["text"].as_str().ok_or("Write a message first.")?;
    let text: String = raw.chars().filter(|ch| !ch.is_control() || *ch == '\n').collect();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Write a message first.".to_string());
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("Keep a message under {MAX_TEXT} characters."));
    }
    Ok(text)
}

/// A message id from a received letter, if it looks like one of ours.
fn received_id(body: &Value) -> Option<String> {
    let id = body["id"].as_str()?;
    (id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| id.to_lowercase())
}

/// Append `{id, from, text, at}` to `owner[key]`, skipping a redelivery and
/// keeping at most [`MAX_MESSAGES`]. Returns true when the list changed.
pub(crate) fn append(owner: &mut Value, key: &str, id: &str, from: &str, text: &str, at: u64) -> bool {
    if !owner[key].is_array() {
        owner[key] = json!([]);
    }
    let Some(list) = owner[key].as_array_mut() else { return false };
    if list.iter().any(|message| message["id"] == id) {
        return false;
    }
    list.push(json!({ "id": id, "from": from, "text": text, "at": at }));
    if list.len() > MAX_MESSAGES {
        list.drain(..list.len() - MAX_MESSAGES);
    }
    true
}

/// The message a received chat letter carries, validated.
pub(crate) fn received(body: &Value) -> Option<(String, String)> {
    let id = received_id(body)?;
    let text = clean_text(body).ok()?;
    Some((id, text))
}

impl Node {
    /// `POST /sales/{id}/message`: the seller writes to this sale's buyer.
    pub(crate) fn send_sale_message(&self, id: &str, body: &Value) -> Result<Value, String> {
        let text = clean_text(body)?;
        let message_id = message_id();
        let now = crate::escrow_outbox::now_secs();
        let sale = self
            .sales()
            .update(id, |sale| {
                append(sale, "messages", &message_id, "seller", &text, now);
            })
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "That sale is not on this node.".to_string())?;
        let reply = json!({ "v": 1, "kind": "chat", "id": message_id, "text": text });
        self.tell_buyer(&sale, &reply).map_err(|err| err.to_string())?;
        self.sale_value(id).ok_or_else(|| "That sale is not on this node.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_trimmed_capped_and_stripped_of_controls() {
        assert_eq!(clean_text(&json!({ "text": "  hi\u{7} there \n" })).unwrap(), "hi there");
        assert!(clean_text(&json!({ "text": "   " })).is_err());
        assert!(clean_text(&json!({ "text": "x".repeat(MAX_TEXT + 1) })).is_err());
        assert!(clean_text(&json!({})).is_err());
    }

    #[test]
    fn append_dedupes_and_keeps_only_the_newest() {
        let mut owner = json!({});
        assert!(append(&mut owner, "messages", "aa", "buyer", "one", 1));
        assert!(!append(&mut owner, "messages", "aa", "buyer", "one again", 2));
        for index in 0..MAX_MESSAGES + 5 {
            append(&mut owner, "messages", &format!("{index:032x}"), "seller", "x", index as u64);
        }
        let list = owner["messages"].as_array().unwrap();
        assert_eq!(list.len(), MAX_MESSAGES);
        assert_eq!(list[0]["text"], "x");
    }

    #[test]
    fn received_messages_need_a_real_id_and_text() {
        let body = json!({ "id": "ab".repeat(16), "text": "hello" });
        assert_eq!(received(&body).unwrap().1, "hello");
        assert!(received(&json!({ "id": "short", "text": "hello" })).is_none());
        assert!(received(&json!({ "id": "ab".repeat(16), "text": " " })).is_none());
    }
}
