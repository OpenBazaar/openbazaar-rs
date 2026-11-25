//! Buyer-only light mode: deterministic 12-word recovery seed, authenticated
//! pull-based order status requests, and local signed contract retention for
//! offline-seller disputes.
//!
//! All key paths reuse [`crate::seed::Seed`] and [`tormail::letter`] so a
//! 12-word BIP 39 phrase created in light mode recovers the exact same order,
//! reply, escrow, and Taproot payout keys in a full node (and vice versa).
//!
//! Safety invariants:
//! - Default network is `testnet4` ([`NetworkMode::Testnet4`]).
//! - Mainnet escrow is strictly refused with [`ESCROW_MAINNET_REFUSAL`].
//! - Escrow is Bitcoin-only (never Monero, Zcash, or Lightning).

use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey, XOnlyPublicKey};
use bitcoin::Address;
use openbazaar_escrow::{BuyerEscrow, Context, FeeKind, Policy, SignedRefund, SignedTerms};
use openbazaar_protocol::plain_text;
use openbazaar_protocol::Store;
use serde_json::{json, Value};
use tormail::letter::derive_secret;
use tormail::{
    open_reply, seal_message, DropId, ReplyPublic, ReplySecret, SealedLetter, SealedReply,
    SenderKey, StoreKey, StoreSecret,
};

use crate::escrow_mail::{hex, key_bytes, parse_id, unhex, EscrowId, Kind, Mail};
use crate::escrow_vendor::{offer_digest, offer_digest_with_listing};
use crate::instructions::ESCROW_MAINNET_REFUSAL;
use crate::network::NetworkMode;
use crate::order_mail::body_bytes;
use crate::seed::Seed;
use crate::Error;

const MAX_DISPUTE_MEMO: usize = 1_000;
const CONTRACT_VERSION: u32 = 1;

/// Deterministic buyer keys derived from a standard 12-word BIP 39 phrase.
pub struct BuyerSeed {
    seed: Seed,
    network: NetworkMode,
}

/// Per-order keys derived deterministically from [`BuyerSeed`], `checkout_id`,
/// and the seller's 32-byte `store_id`.
pub struct BuyerOrderKeys {
    /// BIP-340 per-order signing key (`ob/letter-key/v1`, purpose `"order"`).
    pub sender: SenderKey,
    /// X25519 secret that opens seller replies and pull-status snapshots.
    pub reply_secret: ReplySecret,
    /// X25519 public key sent inside the sealed `OBOD` order.
    pub reply_public: ReplyPublic,
    /// 16-byte drop id (the seller's sale id).
    pub drop_id: DropId,
}

impl BuyerSeed {
    /// Generate a fresh 12-word BIP 39 phrase and derive a [`BuyerSeed`].
    pub fn generate(network: NetworkMode) -> Result<(String, Self), Error> {
        let phrase = Seed::generate_phrase()?;
        let seed = Self::from_phrase(&phrase, network)?;
        Ok((phrase, seed))
    }

    /// Recover a [`BuyerSeed`] from an existing 12-word BIP 39 recovery phrase.
    pub fn from_phrase(phrase: &str, network: NetworkMode) -> Result<Self, Error> {
        let seed = Seed::from_phrase(phrase.trim(), network)?;
        Ok(Self { seed, network })
    }

    pub fn network(&self) -> NetworkMode {
        self.network
    }

    /// Root secret for per-conversation letter keys (`m/1984'/coin'/4'/0'`).
    pub fn letter_root(&self) -> [u8; 32] {
        self.seed.letter_root()
    }

    /// Deterministic per-order signing key, X25519 reply keypair, and drop id.
    /// Matches `crates/node/src/letter_auth.rs` byte-for-byte.
    pub fn order_keys(&self, checkout_id: &str, store_id: &[u8; 32]) -> BuyerOrderKeys {
        let conversation = [checkout_id.as_bytes(), store_id].concat();
        let root = self.seed.letter_root();
        let sender = SenderKey::derive(&root, "order", &conversation);
        let reply_bytes = derive_secret(&root, "reply-key", &conversation, 0);
        let drop_bytes = derive_secret(&root, "reply-drop", &conversation, 0);
        let reply_secret =
            ReplySecret::from_bytes(reply_bytes).expect("any 32 bytes are an X25519 secret");
        let reply_public = reply_secret.public_key();
        let drop_id = DropId(drop_bytes[..16].try_into().expect("16 bytes"));
        BuyerOrderKeys {
            sender,
            reply_secret,
            reply_public,
            drop_id,
        }
    }

    /// Per-order escrow key at `m/1984'/coin'/1'/index'` and its compressed pubkey.
    pub fn escrow_key(&self, index: u32) -> (SecretKey, [u8; 33]) {
        let secret = self.seed.escrow_key(index);
        let pubkey = PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize();
        (secret, pubkey)
    }

    /// Buyer's escrow mailbox X25519 secret (`m/1984'/coin'/2'/0'`).
    pub fn escrow_mail_secret(&self) -> StoreSecret {
        StoreSecret::from_bytes(self.seed.mail_key().secret_bytes())
            .expect("any 32 bytes are an X25519 secret")
    }

    /// Per-escrow BIP-340 letter signing key for buyer role (role byte = 1).
    pub fn escrow_letter_key(&self, escrow_id: &EscrowId) -> SenderKey {
        let conversation = [&[1u8][..], escrow_id].concat();
        SenderKey::derive(&self.seed.letter_root(), "escrow", &conversation)
    }

    /// Deterministic BIP-86 Taproot payout address at `m/86'/coin'/0'/0/{index}`.
    pub fn payout_address(&self, index: u32) -> Address {
        self.seed.receive_address(index)
    }
}

/// Build an authenticated, sealed pull request for `POST /mail/pull`.
///
/// The entire request (sale id, buyer sender key, and fresh anti-replay nonce)
/// travels inside a padded HPKE + BIP-340 [`SealedLetter`] addressed to the
/// seller store's mailbox key.
pub fn build_pull_request(
    drop_id: &[u8; 16],
    sender: &SenderKey,
    store_mailbox: &StoreKey,
    nonce: &[u8; 16],
) -> Result<SealedLetter, String> {
    let payload = json!({
        "v": 1,
        "kind": "pull",
        "sale": hex(drop_id),
        "nonce": hex(nonce),
    });
    seal_message(&body_bytes(&payload), store_mailbox, sender).map_err(|err| err.to_string())
}

/// Open and verify a seller's sealed status response from `POST /mail/pull`.
///
/// Checks:
/// - HPKE decryption with the buyer's per-order [`ReplySecret`].
/// - BIP-340 signature verification of the seller's per-sale key.
/// - Seller key pinning (if `pinned_seller` is already known).
/// - Anti-replay nonce match (`body["nonce"] == hex(expected_nonce)`).
/// - Hostile control / bidi character rejection via [`plain_text::is_plain`].
pub fn open_pull_response(
    reply_bytes: &[u8],
    reply_secret: &ReplySecret,
    expected_nonce: &[u8; 16],
    pinned_seller: Option<&str>,
) -> Result<(String, Value), String> {
    let sealed = SealedReply::from_bytes(reply_bytes).map_err(|_| "that status reply is malformed".to_string())?;
    let opened = open_reply(&sealed, reply_secret).map_err(|_| "that status reply does not open".to_string())?;
    let sender = hex(&opened.sender.0);
    if pinned_seller.is_some_and(|pinned| pinned != sender) {
        return Err("that status reply is not from this order's seller".into());
    }
    let body: Value = serde_json::from_slice(&opened.body)
        .map_err(|_| "that status reply could not be read".to_string())?;
    if body["kind"].as_str() != Some("status") {
        return Err("that reply is not an order status snapshot".into());
    }
    if body["nonce"].as_str() != Some(hex(expected_nonce).as_str()) {
        return Err("that status reply failed its freshness check".into());
    }
    check_pull_text(&body)?;
    Ok((sender, body))
}

/// Refuse hostile control or bidi override characters in untrusted seller status text.
pub(crate) fn check_pull_text(value: &Value) -> Result<(), String> {
    fn visit(val: &Value, multiline: bool) -> Result<(), String> {
        match val {
            Value::String(text) => {
                if !plain_text::is_plain(text, multiline) {
                    return Err(plain_text::REFUSAL.to_string());
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, multiline)?;
                }
            }
            Value::Object(map) => {
                for (key, child) in map {
                    let allow_multi = matches!(
                        key.as_str(),
                        "note" | "content" | "text" | "reason" | "decision" | "returnPolicy"
                    );
                    visit(child, allow_multi)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, false)
}

/// Locally retained escrow artifacts enabling independent address verification
/// and direct-to-moderator disputes even if the seller vanishes offline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedEscrow {
    pub escrow_id: String,
    pub buyer_key_index: u32,
    pub signed_terms_hex: String,
    pub store_signature_hex: String,
    pub store_key_hex: String,
    pub listing_id_hex: Option<String>,
    pub funding_address: String,
    pub funding_outpoint: Option<String>,
    pub funding_tx_hex: Option<String>,
    pub refund_hex: Option<String>,
    pub buyer_snapshot_hex: Option<String>,
    pub moderator_url: String,
    pub moderator_mail_key_hex: String,
    pub refund_script_hex: Option<String>,
}

/// Self-contained signed order & contract record kept locally by the buyer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedContract {
    pub version: u32,
    pub network: String,
    pub order_id: String,
    pub checkout_id: String,
    pub store_id: String,
    pub seller_url: String,
    pub store_record_hex: String,
    pub listing_records_hex: Vec<String>,
    pub escrow: Option<RetainedEscrow>,
}

impl RetainedEscrow {
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "escrowId": self.escrow_id,
            "buyerKeyIndex": self.buyer_key_index,
            "signedTermsHex": self.signed_terms_hex,
            "storeSignatureHex": self.store_signature_hex,
            "storeKeyHex": self.store_key_hex,
            "fundingAddress": self.funding_address,
            "moderatorUrl": self.moderator_url,
            "moderatorMailKeyHex": self.moderator_mail_key_hex,
        });
        if let Some(v) = &self.listing_id_hex { out["listingIdHex"] = json!(v); }
        if let Some(v) = &self.funding_outpoint { out["fundingOutpoint"] = json!(v); }
        if let Some(v) = &self.funding_tx_hex { out["fundingTxHex"] = json!(v); }
        if let Some(v) = &self.refund_hex { out["refundHex"] = json!(v); }
        if let Some(v) = &self.buyer_snapshot_hex { out["buyerSnapshotHex"] = json!(v); }
        if let Some(v) = &self.refund_script_hex { out["refundScriptHex"] = json!(v); }
        out
    }

    pub fn from_json(value: &Value) -> Result<Self, String> {
        let req = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("missing {k}"));
        let opt = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
        let buyer_key_index = value
            .get("buyerKeyIndex")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| "missing buyerKeyIndex".to_string())?;
        Ok(Self {
            escrow_id: req("escrowId")?,
            buyer_key_index,
            signed_terms_hex: req("signedTermsHex")?,
            store_signature_hex: req("storeSignatureHex")?,
            store_key_hex: req("storeKeyHex")?,
            listing_id_hex: opt("listingIdHex"),
            funding_address: req("fundingAddress")?,
            funding_outpoint: opt("fundingOutpoint"),
            funding_tx_hex: opt("fundingTxHex"),
            refund_hex: opt("refundHex"),
            buyer_snapshot_hex: opt("buyerSnapshotHex"),
            moderator_url: req("moderatorUrl")?,
            moderator_mail_key_hex: req("moderatorMailKeyHex")?,
            refund_script_hex: opt("refundScriptHex"),
        })
    }
}

impl RetainedContract {
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "version": self.version,
            "network": self.network,
            "orderId": self.order_id,
            "checkoutId": self.checkout_id,
            "storeId": self.store_id,
            "sellerUrl": self.seller_url,
            "storeRecordHex": self.store_record_hex,
            "listingRecordsHex": self.listing_records_hex,
        });
        if let Some(escrow) = &self.escrow {
            out["escrow"] = escrow.to_json();
        }
        out
    }

    pub fn from_json(value: &Value) -> Result<Self, String> {
        let req = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("missing {k}"));
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| "missing version".to_string())?;
        let listing_records_hex = value
            .get("listingRecordsHex")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let escrow = match value.get("escrow") {
            Some(v) if !v.is_null() => Some(RetainedEscrow::from_json(v)?),
            _ => None,
        };
        Ok(Self {
            version,
            network: req("network")?,
            order_id: req("orderId")?,
            checkout_id: req("checkoutId")?,
            store_id: req("storeId")?,
            seller_url: req("sellerUrl")?,
            store_record_hex: req("storeRecordHex")?,
            listing_records_hex,
            escrow,
        })
    }
}

/// Result of locally verifying a retained escrow contract against the buyer's
/// 12-word seed and the seller's signed store record.
pub struct VerifiedEscrow {
    pub escrow_id: EscrowId,
    pub signed_terms: SignedTerms,
    pub funding_address: Address,
}

/// Dispute letter and sealed mail ready to send straight to the moderator,
/// constructed entirely locally without contacting the seller.
#[derive(Clone, Debug)]
pub struct OfflineDisputePayload {
    pub escrow_id: String,
    pub moderator_url: String,
    pub letter_bytes: Vec<u8>,
    pub sealed_mail_bytes: Vec<u8>,
}

impl RetainedContract {
    /// Verify the signed store record, signed escrow terms, store signature,
    /// buyer key derivation, moderator authorization, and locally computed
    /// MuSig2 Taproot funding address.
    ///
    /// A malicious seller cannot forge or swap the funding address without
    /// failing this check. Mainnet escrow is always refused.
    pub fn verify_escrow(&self, seed: &BuyerSeed) -> Result<VerifiedEscrow, String> {
        if seed.network() != NetworkMode::Testnet4 || self.network != "testnet4" {
            return Err(ESCROW_MAINNET_REFUSAL.to_string());
        }
        let escrow = self
            .escrow
            .as_ref()
            .ok_or_else(|| "This retained contract has no escrow.".to_string())?;
        let escrow_id =
            parse_id(&escrow.escrow_id).ok_or_else(|| "bad retained escrow id".to_string())?;

        // 1. Verify signed store record (`OBST`).
        let store_bytes = unhex(&self.store_record_hex)?;
        let store = Store::decode(&store_bytes).map_err(|err| err.to_string())?;
        store.verify().map_err(|err| err.to_string())?;
        let store_id_hex = store.id.to_hex();
        if store_id_hex != self.store_id || store_id_hex != escrow.store_key_hex {
            return Err("the store record does not match the contract's store key".into());
        }

        // 2. Decode and verify SignedTerms (checks both buyer and vendor BIP-340 sigs).
        let signed_bytes = unhex(&escrow.signed_terms_hex)?;
        let signed = SignedTerms::decode(&signed_bytes).map_err(|err| err.to_string())?;
        let terms = signed.terms();

        // 3. Re-derive buyer escrow pubkey from the 12-word seed and require exact match.
        let (_, expected_buyer) = seed.escrow_key(escrow.buyer_key_index);
        if terms.buyer != expected_buyer {
            return Err("the escrow terms do not match this recovery phrase's buyer key".into());
        }

        // 4. Verify seller's store-key signature over the offer digest.
        let store_sig_bytes = unhex(&escrow.store_signature_hex)?;
        let store_sig = bitcoin::secp256k1::schnorr::Signature::from_slice(&store_sig_bytes)
            .map_err(|_| "bad store signature".to_string())?;
        let store_xonly = XOnlyPublicKey::from_slice(store.id.as_bytes())
            .map_err(|_| "bad store key".to_string())?;
        let digest = match &escrow.listing_id_hex {
            Some(text) => {
                let bytes = unhex(text)?;
                let id: [u8; 16] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| "bad listing id".to_string())?;
                offer_digest_with_listing(&escrow_id, terms, &id)
            }
            None => offer_digest(&escrow_id, terms),
        };
        Secp256k1::verification_only()
            .verify_schnorr(&store_sig, &digest, &store_xonly)
            .map_err(|_| "the escrow offer is not signed by the seller's store key".to_string())?;

        // 5. Verify the moderator in the terms is accepted by the signed store record.
        let mod_xonly: [u8; 32] = terms.moderator.signing_key[1..]
            .try_into()
            .map_err(|_| "bad moderator key".to_string())?;
        if !store
            .details
            .as_ref()
            .is_some_and(|details| details.moderators.contains(&mod_xonly))
        {
            return Err("the seller's signed store record does not accept this moderator".into());
        }

        // 6. Independently derive the MuSig2 Taproot funding output and address.
        let funding = terms.funding_output().map_err(|err| err.to_string())?;
        let derived_address =
            Address::from_script(&funding.script_pubkey, seed.network().bitcoin())
                .map_err(|err| err.to_string())?;
        if derived_address.to_string() != escrow.funding_address {
            return Err(
                "the escrow funding address does not match the signed MuSig2 terms".into(),
            );
        }

        // 7. If a presigned refund is retained, verify it spends the exact funding output.
        if let Some(refund_hex) = &escrow.refund_hex {
            let refund =
                SignedRefund::from_bytes(&unhex(refund_hex)?).map_err(|err| err.to_string())?;
            if refund.funding_output() != &funding {
                return Err("the stored refund does not match the escrow funding output".into());
            }
        }

        Ok(VerifiedEscrow {
            escrow_id,
            signed_terms: signed,
            funding_address: derived_address,
        })
    }

    /// Construct a signed `OBDL` v3 dispute letter and sealed `Kind::Dispute`
    /// mail addressed straight to the moderator, using only this locally
    /// retained contract and the buyer's 12-word recovery seed.
    ///
    /// Works even if the seller disappears and shuts off their node.
    pub fn build_offline_dispute(
        &self,
        seed: &BuyerSeed,
        memo: &str,
        reply_url: &str,
    ) -> Result<OfflineDisputePayload, String> {
        if seed.network() != NetworkMode::Testnet4 || self.network != "testnet4" {
            return Err(ESCROW_MAINNET_REFUSAL.to_string());
        }
        let memo = memo.trim();
        if memo.is_empty() || memo.len() > MAX_DISPUTE_MEMO {
            return Err(format!(
                "Describe the problem in 1 to {MAX_DISPUTE_MEMO} characters."
            ));
        }
        if !plain_text::is_plain(memo, true) {
            return Err(plain_text::REFUSAL.to_string());
        }
        let verified = self.verify_escrow(seed)?;
        let escrow = self
            .escrow
            .as_ref()
            .ok_or_else(|| "This retained contract has no escrow.".to_string())?;
        let snapshot_hex = escrow
            .buyer_snapshot_hex
            .as_deref()
            .ok_or_else(|| "This retained contract has no funded escrow snapshot yet.".to_string())?;
        let snapshot_bytes = unhex(snapshot_hex)?;
        let (buyer_secret, _) = seed.escrow_key(escrow.buyer_key_index);
        let ctx = Context {
            moderator: verified.signed_terms.terms().moderator.clone(),
            policy: Policy::default(),
            tip: 0,
        };
        let restored = BuyerEscrow::restore(&snapshot_bytes, &buyer_secret.secret_bytes(), &ctx)
            .map_err(|err| err.to_string())?;
        let letter_bytes = restored
            .dispute_letter(memo.as_bytes())
            .map_err(|err| err.to_string())?;

        let moderator_url = crate::peer_http::sale_base(&escrow.moderator_url)?;
        let reply_url = crate::peer_http::sale_base(reply_url)?;
        let mod_key = key_bytes(&unhex(&escrow.moderator_mail_key_hex)?)?;
        let buyer_mail_pub = seed.escrow_mail_secret().public_key().as_bytes().to_vec();

        let mut fields = vec![letter_bytes.clone(), buyer_mail_pub, reply_url.into_bytes()];
        if verified.signed_terms.terms().moderator.fee_kind == FeeKind::Filing {
            let refund_script = match &escrow.refund_script_hex {
                Some(script_hex) => unhex(script_hex)?,
                None => seed
                    .payout_address(escrow.buyer_key_index)
                    .script_pubkey()
                    .to_bytes(),
            };
            fields.push(refund_script);
        }
        if let Some(listing_hex) = &escrow.listing_id_hex {
            let listing_bytes = unhex(listing_hex)?;
            let store_key_bytes = unhex(&escrow.store_key_hex)?;
            let store_sig_bytes = unhex(&escrow.store_signature_hex)?;
            if listing_bytes.len() == 16 && store_key_bytes.len() == 32 && store_sig_bytes.len() == 64
            {
                fields.push(listing_bytes);
                fields.push(store_key_bytes);
                fields.push(store_sig_bytes);
            }
        }

        let mail = Mail::new(Kind::Dispute, verified.escrow_id, fields);
        let from = seed.escrow_letter_key(&verified.escrow_id);
        let sealed = seal_message(&mail.encode(), &mod_key, &from).map_err(|err| err.to_string())?;
        Ok(OfflineDisputePayload {
            escrow_id: escrow.escrow_id.clone(),
            moderator_url,
            letter_bytes,
            sealed_mail_bytes: sealed.to_bytes(),
        })
    }
}

impl crate::Node {
    /// Export a locally retained signed contract bundle for one buyer order,
    /// including signed store/listing records and (when present) funded escrow
    /// artifacts so a buyer can dispute directly to the moderator even if the
    /// seller vanishes.
    pub fn export_retained_contract(&self, order_id: &str) -> Result<RetainedContract, String> {
        let order = self
            .orders()
            .get(order_id)
            .cloned()
            .ok_or_else(|| "That order is not on this node.".to_string())?;
        let store_key = crate::stores::unhex32(&order.store_id)
            .ok_or_else(|| "The order names no store key.".to_string())?;
        let (store_record_hex, listing_records_hex) = {
            let catalog = self.catalog();
            let store = catalog
                .stores
                .iter()
                .find(|store| store.id.as_bytes() == &store_key)
                .ok_or_else(|| "This node no longer holds the store's signed record.".to_string())?;
            let store_hex = hex(&store.encode().map_err(|err| err.to_string())?);
            let mut listings = Vec::new();
            for line in &order.lines {
                if let Some(hash) = line["listingHash"].as_str() {
                    if let Some(listing) = crate::find_listing(&catalog, hash) {
                        listings.push(hex(&listing.encode().map_err(|err| err.to_string())?));
                    }
                }
            }
            (store_hex, listings)
        };

        let seller_url = order
            .remote
            .as_ref()
            .and_then(|remote| remote["url"].as_str())
            .map(str::to_string)
            .or_else(|| self.vendor_url(&order.seller).ok())
            .unwrap_or_default();

        let escrow = match &order.escrow {
            Some(escrow_val) => {
                if self.network != NetworkMode::Testnet4 {
                    return Err(ESCROW_MAINNET_REFUSAL.to_string());
                }
                let escrow_id_str = escrow_val["id"]
                    .as_str()
                    .ok_or_else(|| "This escrow has no id.".to_string())?
                    .to_string();
                let escrow_id =
                    parse_id(&escrow_id_str).ok_or_else(|| "bad escrow id".to_string())?;
                let buyer_key_index = escrow_val["buyerKeyIndex"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| "This escrow has no buyer key index.".to_string())?;
                let signed_terms_hex = escrow_val["terms"]
                    .as_str()
                    .ok_or_else(|| "This escrow has no signed terms yet.".to_string())?
                    .to_string();
                let store_signature_hex = escrow_val["storeSignature"]
                    .as_str()
                    .ok_or_else(|| "This escrow has no store signature.".to_string())?
                    .to_string();
                let store_key_hex = escrow_val["storeKey"]
                    .as_str()
                    .unwrap_or(&order.store_id)
                    .to_string();
                let funding_address = escrow_val["address"]
                    .as_str()
                    .ok_or_else(|| "This escrow has no funding address yet.".to_string())?
                    .to_string();
                let (moderator_url, moderator_mail_key) = self.order_moderator_mailbox(escrow_val)?;
                let buyer_snapshot_hex = self
                    .escrow
                    .store
                    .load(crate::escrow_store::Role::Buyer, &crate::escrow_store::name(&escrow_id))?
                    .and_then(|rec| rec["snapshot"].as_str().map(str::to_string));
                Some(RetainedEscrow {
                    escrow_id: escrow_id_str,
                    buyer_key_index,
                    signed_terms_hex,
                    store_signature_hex,
                    store_key_hex,
                    listing_id_hex: escrow_val["listingId"].as_str().map(str::to_string),
                    funding_address,
                    funding_outpoint: escrow_val["outpoint"].as_str().map(str::to_string),
                    funding_tx_hex: escrow_val["fundingTx"].as_str().map(str::to_string),
                    refund_hex: escrow_val["refund"].as_str().map(str::to_string),
                    buyer_snapshot_hex,
                    moderator_url,
                    moderator_mail_key_hex: hex(moderator_mail_key.as_bytes()),
                    refund_script_hex: None,
                })
            }
            None => None,
        };

        Ok(RetainedContract {
            version: CONTRACT_VERSION,
            network: self.network.as_str().to_string(),
            order_id: order.id,
            checkout_id: order.checkout_id,
            store_id: order.store_id,
            seller_url,
            store_record_hex,
            listing_records_hex,
            escrow,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn buyer_seed_recovers_identical_order_escrow_and_payout_keys_and_separates_networks() {
        let first = BuyerSeed::from_phrase(TEST_PHRASE, NetworkMode::Testnet4).unwrap();
        let recovered = BuyerSeed::from_phrase(TEST_PHRASE, NetworkMode::Testnet4).unwrap();
        let mainnet = BuyerSeed::from_phrase(TEST_PHRASE, NetworkMode::Mainnet).unwrap();
        let store = [0x42u8; 32];

        let k1 = first.order_keys("checkout-001", &store);
        let k2 = recovered.order_keys("checkout-001", &store);
        let km = mainnet.order_keys("checkout-001", &store);
        assert_eq!(k1.sender.id(), k2.sender.id());
        assert_eq!(k1.reply_public.as_bytes(), k2.reply_public.as_bytes());
        assert_eq!(k1.drop_id, k2.drop_id);
        assert_ne!(k1.sender.id(), km.sender.id(), "networks must not share order keys");
        assert_ne!(k1.drop_id, km.drop_id);

        let (e1, p1) = first.escrow_key(0);
        let (e2, p2) = recovered.escrow_key(0);
        let (em, _) = mainnet.escrow_key(0);
        assert_eq!((e1, p1), (e2, p2));
        assert_ne!(e1, em, "networks must not share escrow keys");
        assert_ne!(first.escrow_key(0).1, first.escrow_key(1).1);

        let a1 = first.payout_address(0);
        let a2 = recovered.payout_address(0);
        let am = mainnet.payout_address(0);
        assert_eq!(a1, a2);
        assert!(a1.to_string().starts_with("tb1p"), "{a1}");
        assert!(am.to_string().starts_with("bc1p"), "{am}");

        // Cross-check against a fresh generated 12-word phrase.
        let (phrase, generated) = BuyerSeed::generate(NetworkMode::Testnet4).unwrap();
        assert_eq!(phrase.split_whitespace().count(), 12);
        let again = BuyerSeed::from_phrase(&phrase, NetworkMode::Testnet4).unwrap();
        assert_eq!(generated.escrow_key(3), again.escrow_key(3));
        assert_eq!(generated.payout_address(2), again.payout_address(2));
    }

    #[test]
    fn buyer_seed_matches_node_wallet_receive_address() {
        let dir = crate::harness::TempDir::new();
        let vault = crate::vault::Vault::from_key(&[9u8; 32]);
        let wallet_dir = dir.0.join("wallets").join("node");
        std::fs::create_dir_all(&wallet_dir).unwrap();
        vault
            .write_sealed(&wallet_dir.join("seed.obv"), TEST_PHRASE.as_bytes())
            .unwrap();
        let mut wallet =
            crate::wallet::NodeWallet::open(&dir.0.join("wallets"), "node", &vault, NetworkMode::Testnet4)
                .unwrap();
        let buyer_seed = BuyerSeed::from_phrase(TEST_PHRASE, NetworkMode::Testnet4).unwrap();
        assert_eq!(wallet.fresh_address().unwrap(), buyer_seed.payout_address(0));
        assert_eq!(wallet.fresh_address().unwrap(), buyer_seed.payout_address(1));
    }

    #[test]
    fn mainnet_retained_escrow_always_refuses_with_escrow_mainnet_refusal() {
        let main_seed = BuyerSeed::from_phrase(TEST_PHRASE, NetworkMode::Mainnet).unwrap();
        let contract = RetainedContract {
            version: CONTRACT_VERSION,
            network: "mainnet".to_string(),
            order_id: "o1".into(),
            checkout_id: "c1".into(),
            store_id: "aa".repeat(32),
            seller_url: "http://127.0.0.1:9999".into(),
            store_record_hex: String::new(),
            listing_records_hex: vec![],
            escrow: None,
        };
        assert_eq!(
            contract.verify_escrow(&main_seed).err().as_deref(),
            Some(ESCROW_MAINNET_REFUSAL)
        );
        assert_eq!(
            contract
                .build_offline_dispute(&main_seed, "never shipped", "http://127.0.0.1:9998")
                .err()
                .as_deref(),
            Some(ESCROW_MAINNET_REFUSAL)
        );
    }

    #[test]
    fn pull_response_rejects_hostile_text_wrong_nonce_and_unpinned_seller() {
        let seed = BuyerSeed::from_phrase(TEST_PHRASE, NetworkMode::Testnet4).unwrap();
        let keys = seed.order_keys("chk-1", &[7u8; 32]);
        let seller = SenderKey::generate();
        let other_seller = SenderKey::generate();
        let nonce = [0x11u8; 16];
        let good = json!({ "v": 1, "kind": "status", "nonce": hex(&nonce), "status": "Paid" });
        let sealed = tormail::seal_reply(&body_bytes(&good), &keys.reply_public, &seller).unwrap();
        let (sender_hex, parsed) =
            open_pull_response(&sealed.to_bytes(), &keys.reply_secret, &nonce, None).unwrap();
        assert_eq!(parsed["status"], "Paid");

        // Wrong nonce refused.
        assert!(open_pull_response(&sealed.to_bytes(), &keys.reply_secret, &[0x22u8; 16], Some(&sender_hex)).is_err());

        // Different seller refused once pinned.
        let spoof = tormail::seal_reply(&body_bytes(&good), &keys.reply_public, &other_seller).unwrap();
        assert!(open_pull_response(&spoof.to_bytes(), &keys.reply_secret, &nonce, Some(&sender_hex)).is_err());

        // Hostile bidi text refused.
        let hostile = json!({
            "v": 1,
            "kind": "status",
            "nonce": hex(&nonce),
            "fulfillment": { "note": "shipped \u{202e}fake" }
        });
        let sealed_hostile = tormail::seal_reply(&body_bytes(&hostile), &keys.reply_public, &seller).unwrap();
        assert!(open_pull_response(&sealed_hostile.to_bytes(), &keys.reply_secret, &nonce, None).is_err());
    }
}
