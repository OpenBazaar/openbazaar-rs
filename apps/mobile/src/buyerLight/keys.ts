// Deterministic buyer-only light keys from a standard 12-word BIP 39 seed.
//
// Derivation paths match `crates/node/src/seed.rs` and `crates/node/src/letter_auth.rs`
// byte-for-byte so a 12-word phrase recovers the exact same order, reply,
// escrow, and Taproot payout keys across both mobile light mode and `ob-node`:
//
//   m/86'/coin'/0'/0/{index}   BIP-86 Taproot payout address
//   m/1984'/coin'/1'/{index}'  per-order escrow key
//   m/1984'/coin'/2'/0'        escrow mail X25519 key
//   m/1984'/coin'/4'/0'        letter root (per-order signing, reply, drop id)
//
// `coin` is 1 on testnet4 (default) and 0 on mainnet.

import { x25519 } from "@noble/curves/ed25519.js";
import { schnorr, secp256k1 } from "@noble/curves/secp256k1.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256, sha512 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";
import { bech32m } from "@scure/base";
import { generateMnemonic, mnemonicToSeedSync, validateMnemonic } from "@scure/bip39";
import { wordlist } from "@scure/bip39/wordlists/english.js";

export type BuyerNetwork = "testnet4" | "mainnet";

export const DEFAULT_BUYER_NETWORK: BuyerNetwork = "testnet4";

const HARDENED = 0x80000000;
const SECP_N = secp256k1.Point.Fn.ORDER;
const LETTER_DERIVE_TAG = "ob/letter-key/v1";

type Bip32Node = { key: Uint8Array; chainCode: Uint8Array };

export type BuyerSeed = {
  readonly network: BuyerNetwork;
  readonly master: Bip32Node;
  readonly letterRoot: Uint8Array;
};

export type BuyerOrderKeys = {
  readonly senderSecret: Uint8Array;
  readonly senderPublicHex: string;
  readonly replySecret: Uint8Array;
  readonly replyPublic: Uint8Array;
  readonly replyPublicHex: string;
  readonly dropId: Uint8Array;
  readonly dropIdHex: string;
};

export type BuyerEscrowKey = {
  readonly index: number;
  readonly secret: Uint8Array;
  readonly pubkeyCompressed: Uint8Array;
  readonly pubkeyCompressedHex: string;
};

function u32be(value: number): Uint8Array {
  const out = new Uint8Array(4);
  out[0] = (value >>> 24) & 0xff;
  out[1] = (value >>> 16) & 0xff;
  out[2] = (value >>> 8) & 0xff;
  out[3] = value & 0xff;
  return out;
}

function u16le(value: number): Uint8Array {
  return new Uint8Array([value & 0xff, (value >>> 8) & 0xff]);
}

function bytesToBigInt(bytes: Uint8Array): bigint {
  let out = 0n;
  for (const byte of bytes) out = (out << 8n) | BigInt(byte);
  return out;
}

function bigIntToBytes32(value: bigint): Uint8Array {
  const out = new Uint8Array(32);
  let cur = value;
  for (let i = 31; i >= 0; i--) {
    out[i] = Number(cur & 0xffn);
    cur >>= 8n;
  }
  return out;
}

export function taggedHash(tag: string, ...parts: Uint8Array[]): Uint8Array {
  const tagDigest = sha256(utf8ToBytes(tag));
  return sha256(concatBytes(tagDigest, tagDigest, ...parts));
}

function bip32Master(seedBytes: Uint8Array): Bip32Node {
  const digest = hmac(sha512, utf8ToBytes("Bitcoin seed"), seedBytes);
  const key = digest.subarray(0, 32);
  const chainCode = digest.subarray(32, 64);
  const scalar = bytesToBigInt(key);
  if (scalar === 0n || scalar >= SECP_N) throw new Error("invalid master key");
  return { key: new Uint8Array(key), chainCode: new Uint8Array(chainCode) };
}

function bip32Child(node: Bip32Node, index: number, hardened: boolean): Bip32Node {
  const childIdx = hardened ? (index | HARDENED) >>> 0 : index >>> 0;
  const data = hardened
    ? concatBytes(new Uint8Array([0]), node.key, u32be(childIdx))
    : concatBytes(secp256k1.getPublicKey(node.key, true), u32be(childIdx));
  const digest = hmac(sha512, node.chainCode, data);
  const il = bytesToBigInt(digest.subarray(0, 32));
  if (il >= SECP_N) throw new Error("invalid child tweak");
  const childScalar = (il + bytesToBigInt(node.key)) % SECP_N;
  if (childScalar === 0n) throw new Error("invalid zero child key");
  return {
    key: bigIntToBytes32(childScalar),
    chainCode: new Uint8Array(digest.subarray(32, 64)),
  };
}

function derivePath(master: Bip32Node, segments: readonly [number, boolean][]): Uint8Array {
  let current = master;
  for (const [index, hardened] of segments) {
    current = bip32Child(current, index, hardened);
  }
  return current.key;
}

function coinType(network: BuyerNetwork): number {
  return network === "mainnet" ? 0 : 1;
}

/** Normalize whitespace and casing for a 12-word BIP 39 recovery phrase. */
export function normalizeBuyerPhrase(phrase: string): string {
  return phrase.trim().toLowerCase().replace(/\s+/g, " ");
}

/** Generate a standard 12-word (128-bit entropy) BIP 39 recovery phrase. */
export function generateBuyerPhrase(): string {
  return generateMnemonic(wordlist, 128);
}

/** True when `phrase` is a valid 12-word BIP 39 English mnemonic. */
export function isValidBuyerPhrase(phrase: string): boolean {
  const clean = normalizeBuyerPhrase(phrase);
  if (clean.split(" ").length !== 12) return false;
  return validateMnemonic(clean, wordlist);
}

/** Derive a deterministic buyer seed from a 12-word BIP 39 phrase. Defaults to `testnet4`. */
export function buyerSeedFromPhrase(phrase: string, network: BuyerNetwork = DEFAULT_BUYER_NETWORK): BuyerSeed {
  const clean = normalizeBuyerPhrase(phrase);
  if (!isValidBuyerPhrase(clean)) {
    throw new Error("Enter a valid 12-word BIP 39 recovery phrase.");
  }
  const bip39Seed = mnemonicToSeedSync(clean, "");
  const master = bip32Master(bip39Seed);
  const coin = coinType(network);
  const letterRoot = derivePath(master, [
    [1984, true],
    [coin, true],
    [4, true],
    [0, true],
  ]);
  return { network, master, letterRoot };
}

/** `crates/tormail/src/letter.rs::derive_secret` byte-for-byte. */
export function deriveLetterSecret(root: Uint8Array, purpose: string, conversation: Uint8Array, counter = 0): Uint8Array {
  const purposeBytes = utf8ToBytes(purpose);
  return taggedHash(
    LETTER_DERIVE_TAG,
    root,
    u16le(purposeBytes.length),
    purposeBytes,
    conversation,
    new Uint8Array([counter & 0xff]),
  );
}

function deriveSenderSecret(root: Uint8Array, purpose: string, conversation: Uint8Array): Uint8Array {
  for (let counter = 0; counter < 16; counter++) {
    const candidate = deriveLetterSecret(root, purpose, conversation, counter);
    const scalar = bytesToBigInt(candidate);
    if (scalar > 0n && scalar < SECP_N) return candidate;
  }
  throw new Error("could not derive valid sender key");
}

/** Deterministic per-order signing key, X25519 reply keypair, and 16-byte drop id. */
export function deriveOrderKeys(seed: BuyerSeed, checkoutId: string, storeIdHex: string): BuyerOrderKeys {
  if (!/^[0-9a-f]{64}$/i.test(storeIdHex)) {
    throw new Error("Invalid store key.");
  }
  const conversation = concatBytes(utf8ToBytes(checkoutId), hexToBytes(storeIdHex.toLowerCase()));
  const senderSecret = deriveSenderSecret(seed.letterRoot, "order", conversation);
  const senderPublicHex = bytesToHex(schnorr.getPublicKey(senderSecret));
  const replySecret = deriveLetterSecret(seed.letterRoot, "reply-key", conversation, 0);
  const replyPublic = x25519.getPublicKey(replySecret);
  const dropId = deriveLetterSecret(seed.letterRoot, "reply-drop", conversation, 0).slice(0, 16);
  return {
    senderSecret,
    senderPublicHex,
    replySecret,
    replyPublic,
    replyPublicHex: bytesToHex(replyPublic),
    dropId,
    dropIdHex: bytesToHex(dropId),
  };
}

/** Per-order escrow key at `m/1984'/coin'/1'/{index}'`. */
export function deriveEscrowKey(seed: BuyerSeed, index: number): BuyerEscrowKey {
  if (!Number.isInteger(index) || index < 0 || index >= HARDENED) {
    throw new Error("Escrow key index out of range.");
  }
  const secret = derivePath(seed.master, [
    [1984, true],
    [coinType(seed.network), true],
    [1, true],
    [index, true],
  ]);
  const pubkeyCompressed = secp256k1.getPublicKey(secret, true);
  return {
    index,
    secret,
    pubkeyCompressed,
    pubkeyCompressedHex: bytesToHex(pubkeyCompressed),
  };
}

/** Buyer's escrow mailbox X25519 key at `m/1984'/coin'/2'/0'`. */
export function deriveEscrowMailKey(seed: BuyerSeed): { secret: Uint8Array; publicKey: Uint8Array; publicHex: string } {
  const secret = derivePath(seed.master, [
    [1984, true],
    [coinType(seed.network), true],
    [2, true],
    [0, true],
  ]);
  const publicKey = x25519.getPublicKey(secret);
  return { secret, publicKey, publicHex: bytesToHex(publicKey) };
}

/** Buyer's per-escrow letter signing key (role byte 1). */
export function deriveEscrowLetterKey(seed: BuyerSeed, escrowIdHex: string): { secret: Uint8Array; publicHex: string } {
  if (!/^[0-9a-f]{32}$/i.test(escrowIdHex)) throw new Error("Invalid escrow id.");
  const conversation = concatBytes(new Uint8Array([1]), hexToBytes(escrowIdHex.toLowerCase()));
  const secret = deriveSenderSecret(seed.letterRoot, "escrow", conversation);
  return { secret, publicHex: bytesToHex(schnorr.getPublicKey(secret)) };
}

/** Taproot tweaked x-only key from an internal x-only public key (BIP 341, no script tree). */
export function taprootTweakXOnly(internalXOnly: Uint8Array): Uint8Array {
  const internalPoint = secp256k1.Point.fromBytes(concatBytes(new Uint8Array([0x02]), internalXOnly));
  const tweak = bytesToBigInt(taggedHash("TapTweak", internalXOnly));
  if (tweak >= SECP_N) throw new Error("invalid TapTweak");
  const tweaked = internalPoint.add(secp256k1.Point.BASE.multiply(tweak));
  return bigIntToBytes32(tweaked.x);
}

/** Encode a 32-byte Taproot output key as a Bech32m address (`tb1p...` on testnet4, `bc1p...` on mainnet). */
export function encodeTaprootAddress(outputXOnly: Uint8Array, network: BuyerNetwork): string {
  if (outputXOnly.length !== 32) throw new Error("Taproot output key must be 32 bytes.");
  const hrp = network === "mainnet" ? "bc" : "tb";
  return bech32m.encode(hrp, [1, ...bech32m.toWords(outputXOnly)]);
}

/** Deterministic BIP-86 Taproot payout address at `m/86'/coin'/0'/0/{index}`. */
export function derivePayoutAddress(seed: BuyerSeed, index: number): string {
  if (!Number.isInteger(index) || index < 0 || index >= HARDENED) {
    throw new Error("Payout index out of range.");
  }
  const secret = derivePath(seed.master, [
    [86, true],
    [coinType(seed.network), true],
    [0, true],
    [0, false],
    [index, false],
  ]);
  const internalXOnly = schnorr.getPublicKey(secret);
  const outputXOnly = taprootTweakXOnly(internalXOnly);
  return encodeTaprootAddress(outputXOnly, seed.network);
}
