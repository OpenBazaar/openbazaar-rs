// Wire-compatible tormail sealed order, sealed message, and sealed reply
// implementation for signed app code (never run in seller-served web JS).
//
// Matches `crates/tormail`:
// - RFC 9180 HPKE Base mode: DHKEM(X25519, HKDF-SHA256) + HKDF-SHA256 + ChaCha20Poly1305
// - Padded BIP-340 signed envelope (`OBLT` v1, tag `ob/letter/v1`)
// - Sealed letter (`OBSL` v2, buckets [2048, 8192, 65536])
// - Sealed reply (`OBSR` v2, buckets [2048, 8192])
// - Order payload (`OBOD` v2)

import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { schnorr } from "@noble/curves/secp256k1.js";
import { expand, extract } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { taggedHash } from "./keys.ts";

const HPKE_VERSION = utf8ToBytes("HPKE-v1");
const KEM_SUITE_ID = concatBytes(utf8ToBytes("KEM"), new Uint8Array([0x00, 0x20]));
const HPKE_SUITE_ID = concatBytes(
  utf8ToBytes("HPKE"),
  new Uint8Array([0x00, 0x20, 0x00, 0x01, 0x00, 0x03]),
);

const ORDER_INFO = utf8ToBytes("openbazaar-tormail-order-v1");
const MESSAGE_INFO = utf8ToBytes("openbazaar-tormail-message-v1");
const REPLY_INFO = utf8ToBytes("openbazaar-tormail-reply-v1");

const OBLT_MAGIC = utf8ToBytes("OBLT");
const OBSL_MAGIC = utf8ToBytes("OBSL");
const OBSR_MAGIC = utf8ToBytes("OBSR");
const OBOD_MAGIC = utf8ToBytes("OBOD");

const SEAL_VERSION = 2;
const HEADER_LEN = 4 + 1 + 32 + 4;
const TAG_LEN = 16;
const OBLT_OVERHEAD = 4 + 1 + 1 + 32 + 16 + 4 + 64;
const LETTER_BUCKETS = [2048, 8192, 65536] as const;
const REPLY_BUCKETS = [2048, 8192] as const;

const KIND_ORDER = 1;
const KIND_MESSAGE = 2;
const KIND_REPLY = 3;

function u16be(n: number): Uint8Array {
  return new Uint8Array([(n >>> 8) & 0xff, n & 0xff]);
}

function u16le(n: number): Uint8Array {
  return new Uint8Array([n & 0xff, (n >>> 8) & 0xff]);
}

function u32le(n: number): Uint8Array {
  return new Uint8Array([n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff]);
}

function readU32le(bytes: Uint8Array, at: number): number {
  return (bytes[at] | (bytes[at + 1] << 8) | (bytes[at + 2] << 16)) + bytes[at + 3] * 0x1000000;
}

function labeledExtract(suiteId: Uint8Array, salt: Uint8Array, label: string, ikm: Uint8Array): Uint8Array {
  return extract(sha256, concatBytes(HPKE_VERSION, suiteId, utf8ToBytes(label), ikm), salt);
}

function labeledExpand(suiteId: Uint8Array, prk: Uint8Array, label: string, info: Uint8Array, length: number): Uint8Array {
  return expand(sha256, prk, concatBytes(u16be(length), HPKE_VERSION, suiteId, utf8ToBytes(label), info), length);
}

function keyScheduleBase(sharedSecret: Uint8Array, info: Uint8Array): { key: Uint8Array; nonce: Uint8Array } {
  const empty = new Uint8Array(0);
  const pskIdHash = labeledExtract(HPKE_SUITE_ID, empty, "psk_id_hash", empty);
  const infoHash = labeledExtract(HPKE_SUITE_ID, empty, "info_hash", info);
  const ksContext = concatBytes(new Uint8Array([0x00]), pskIdHash, infoHash);
  const secret = labeledExtract(HPKE_SUITE_ID, sharedSecret, "secret", empty);
  const key = labeledExpand(HPKE_SUITE_ID, secret, "key", ksContext, 32);
  const nonce = labeledExpand(HPKE_SUITE_ID, secret, "base_nonce", ksContext, 12);
  return { key, nonce };
}

/** RFC 9180 HPKE Base Seal (X25519 + HKDF-SHA256 + ChaCha20Poly1305). */
export function hpkeSealBase(recipientPub: Uint8Array, info: Uint8Array, plaintext: Uint8Array): { enc: Uint8Array; ciphertext: Uint8Array } {
  if (recipientPub.length !== 32) throw new Error("Recipient key must be 32 bytes.");
  const skE = randomBytes(32);
  const enc = x25519.getPublicKey(skE);
  const dh = x25519.getSharedSecret(skE, recipientPub);
  const kemContext = concatBytes(enc, recipientPub);
  const eaePrk = labeledExtract(KEM_SUITE_ID, new Uint8Array(0), "eae_prk", dh);
  const sharedSecret = labeledExpand(KEM_SUITE_ID, eaePrk, "shared_secret", kemContext, 32);
  const { key, nonce } = keyScheduleBase(sharedSecret, info);
  const ciphertext = chacha20poly1305(key, nonce, new Uint8Array(0)).encrypt(plaintext);
  return { enc, ciphertext };
}

/** RFC 9180 HPKE Base Open (X25519 + HKDF-SHA256 + ChaCha20Poly1305). */
export function hpkeOpenBase(recipientSecret: Uint8Array, enc: Uint8Array, info: Uint8Array, ciphertext: Uint8Array): Uint8Array {
  if (recipientSecret.length !== 32 || enc.length !== 32) throw new Error("Bad HPKE key length.");
  const pkR = x25519.getPublicKey(recipientSecret);
  const dh = x25519.getSharedSecret(recipientSecret, enc);
  const kemContext = concatBytes(enc, pkR);
  const eaePrk = labeledExtract(KEM_SUITE_ID, new Uint8Array(0), "eae_prk", dh);
  const sharedSecret = labeledExpand(KEM_SUITE_ID, eaePrk, "shared_secret", kemContext, 32);
  const { key, nonce } = keyScheduleBase(sharedSecret, info);
  return chacha20poly1305(key, nonce, new Uint8Array(0)).decrypt(ciphertext);
}

function letterDigest(kind: number, recipient: Uint8Array, sender: Uint8Array, messageId: Uint8Array, body: Uint8Array): Uint8Array {
  return taggedHash(
    "ob/letter/v1",
    recipient,
    new Uint8Array([kind]),
    sender,
    messageId,
    u32le(body.length),
    body,
  );
}

function encodeSignedEnvelope(kind: number, recipient: Uint8Array, senderSecret: Uint8Array, body: Uint8Array, paddedLen: number): Uint8Array {
  if (OBLT_OVERHEAD + body.length > paddedLen) {
    throw new Error("Letter body does not fit its size bucket.");
  }
  const senderPub = schnorr.getPublicKey(senderSecret);
  const messageId = randomBytes(16);
  const digest = letterDigest(kind, recipient, senderPub, messageId, body);
  const signature = schnorr.sign(digest, senderSecret, new Uint8Array(32));
  const out = new Uint8Array(paddedLen);
  let pos = 0;
  out.set(OBLT_MAGIC, pos); pos += 4;
  out[pos++] = 1;
  out[pos++] = kind;
  out.set(senderPub, pos); pos += 32;
  out.set(messageId, pos); pos += 16;
  out.set(u32le(body.length), pos); pos += 4;
  out.set(body, pos); pos += body.length;
  out.set(signature, pos);
  return out;
}

function decodeSignedEnvelope(kind: number, recipient: Uint8Array, plaintext: Uint8Array): { senderHex: string; messageIdHex: string; body: Uint8Array } {
  if (plaintext.length < OBLT_OVERHEAD) throw new Error("Envelope too short.");
  for (let i = 0; i < 4; i++) if (plaintext[i] !== OBLT_MAGIC[i]) throw new Error("Bad envelope magic.");
  if (plaintext[4] !== 1 || plaintext[5] !== kind) throw new Error("Bad envelope version or kind.");
  const sender = plaintext.subarray(6, 38);
  const messageId = plaintext.subarray(38, 54);
  const bodyLen = readU32le(plaintext, 54);
  const bodyEnd = 58 + bodyLen;
  const sigEnd = bodyEnd + 64;
  if (sigEnd > plaintext.length) throw new Error("Envelope truncated.");
  for (let i = sigEnd; i < plaintext.length; i++) {
    if (plaintext[i] !== 0) throw new Error("Non-zero padding.");
  }
  const body = plaintext.subarray(58, bodyEnd);
  const signature = plaintext.subarray(bodyEnd, sigEnd);
  const digest = letterDigest(kind, recipient, sender, messageId, body);
  if (!schnorr.verify(signature, digest, sender)) {
    throw new Error("Envelope signature failed.");
  }
  return {
    senderHex: bytesToHex(sender),
    messageIdHex: bytesToHex(messageId),
    body: new Uint8Array(body),
  };
}

function encodeBlob(magic: Uint8Array, enc: Uint8Array, ciphertext: Uint8Array): Uint8Array {
  return concatBytes(magic, new Uint8Array([SEAL_VERSION]), enc, u32le(ciphertext.length), ciphertext);
}

function decodeBlob(magic: Uint8Array, buckets: readonly number[], bytes: Uint8Array): { enc: Uint8Array; ciphertext: Uint8Array } {
  if (bytes.length < HEADER_LEN || !buckets.includes(bytes.length)) {
    throw new Error("Malformed sealed blob size.");
  }
  for (let i = 0; i < 4; i++) if (bytes[i] !== magic[i]) throw new Error("Bad sealed blob magic.");
  if (bytes[4] !== SEAL_VERSION) throw new Error("Unsupported sealed blob version.");
  const enc = bytes.subarray(5, 37);
  const len = readU32le(bytes, 37);
  const ciphertext = bytes.subarray(41);
  if (ciphertext.length !== len || ciphertext.length < TAG_LEN) {
    throw new Error("Truncated sealed blob.");
  }
  return { enc, ciphertext };
}

function sealPadded(magic: Uint8Array, buckets: readonly number[], info: Uint8Array, kind: number, recipient: Uint8Array, senderSecret: Uint8Array, body: Uint8Array): Uint8Array {
  const needed = HEADER_LEN + TAG_LEN + OBLT_OVERHEAD + body.length;
  const bucket = buckets.find((size) => size >= needed);
  if (!bucket) throw new Error("Letter exceeds maximum size bucket.");
  const plain = encodeSignedEnvelope(kind, recipient, senderSecret, body, bucket - HEADER_LEN - TAG_LEN);
  const { enc, ciphertext } = hpkeSealBase(recipient, info, plain);
  return encodeBlob(magic, enc, ciphertext);
}

function pushStr(parts: Uint8Array[], value: string): void {
  const bytes = utf8ToBytes(value);
  if (bytes.length > 0xffff) throw new Error("Text field too long.");
  parts.push(u16le(bytes.length), bytes);
}

export type LightOrderPayload = {
  readonly listingHashHex: string;
  readonly quantity: number;
  readonly options: Record<string, string>;
  readonly shipTo: string;
  readonly rail: "lightning" | "onchain";
  readonly replyPublic: Uint8Array;
  readonly dropId: Uint8Array;
};

/** Encode a v2 `tormail::Order` (`OBOD`) matching `crates/tormail/src/order.rs`. */
export function encodeOrder(order: LightOrderPayload): Uint8Array {
  if (!/^[0-9a-f]{64}$/i.test(order.listingHashHex)) throw new Error("Invalid listing hash.");
  if (!Number.isInteger(order.quantity) || order.quantity < 1 || order.quantity > 99) {
    throw new Error("Quantity must be between 1 and 99.");
  }
  if (!order.shipTo.trim()) throw new Error("Ship-to or delivery note is required.");
  if (order.replyPublic.length !== 32 || order.dropId.length !== 16) {
    throw new Error("Invalid reply path.");
  }
  const entries = Object.entries(order.options).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const parts: Uint8Array[] = [
    OBOD_MAGIC,
    new Uint8Array([2]),
    hexToBytes(order.listingHashHex.toLowerCase()),
    u32le(order.quantity),
    u16le(entries.length),
  ];
  for (const [name, value] of entries) {
    pushStr(parts, name);
    pushStr(parts, value);
  }
  pushStr(parts, order.shipTo);
  parts.push(new Uint8Array([order.rail === "lightning" ? 1 : 2]));
  parts.push(order.replyPublic);
  pushStr(parts, ""); // pull-only light buyer has no 24/7 onion reply mailbox
  parts.push(order.dropId);
  return concatBytes(...parts);
}

/** Seal an `OBOD` order to the seller store's X25519 mailbox key. */
export function sealOrderLetter(order: LightOrderPayload, storeMailboxHex: string, buyerSenderSecret: Uint8Array): Uint8Array {
  if (!/^[0-9a-f]{64}$/i.test(storeMailboxHex)) throw new Error("Invalid store mailbox key.");
  return sealPadded(
    OBSL_MAGIC,
    LETTER_BUCKETS,
    ORDER_INFO,
    KIND_ORDER,
    hexToBytes(storeMailboxHex.toLowerCase()),
    buyerSenderSecret,
    encodeOrder(order),
  );
}

/** Seal an opaque JSON message (pull request, chat, complete) to the seller store's mailbox key. */
export function sealMessageLetter(body: Uint8Array, storeMailboxHex: string, buyerSenderSecret: Uint8Array): Uint8Array {
  if (!/^[0-9a-f]{64}$/i.test(storeMailboxHex)) throw new Error("Invalid store mailbox key.");
  return sealPadded(
    OBSL_MAGIC,
    LETTER_BUCKETS,
    MESSAGE_INFO,
    KIND_MESSAGE,
    hexToBytes(storeMailboxHex.toLowerCase()),
    buyerSenderSecret,
    body,
  );
}

/** Seal a reply (`OBSR`) to a buyer's reply public key (used in round-trip tests). */
export function sealReplyForTest(body: Uint8Array, buyerReplyPub: Uint8Array, sellerSenderSecret: Uint8Array): Uint8Array {
  return sealPadded(OBSR_MAGIC, REPLY_BUCKETS, REPLY_INFO, KIND_REPLY, buyerReplyPub, sellerSenderSecret, body);
}

/** Open and verify a seller's `OBSR` sealed reply with the buyer's per-order reply secret. */
export function openSealedReply(replyBytes: Uint8Array, replySecret: Uint8Array): { senderHex: string; messageIdHex: string; body: Uint8Array } {
  const { enc, ciphertext } = decodeBlob(OBSR_MAGIC, REPLY_BUCKETS, replyBytes);
  const plaintext = hpkeOpenBase(replySecret, enc, REPLY_INFO, ciphertext);
  const recipientPub = x25519.getPublicKey(replySecret);
  return decodeSignedEnvelope(KIND_REPLY, recipientPub, plaintext);
}
