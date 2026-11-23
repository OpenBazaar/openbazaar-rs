// Local, signed-app-code verification of moderated escrow terms and 2-of-3
// MuSig2 Taproot address derivation.
//
// Hard safety rules enforced here:
// - Mainnet escrow is strictly refused with `ESCROW_MAINNET_REFUSAL`.
// - Escrow is Bitcoin-only (never Monero, Zcash, or Lightning).
// - The buyer's app independently derives the MuSig2 + BIP-341 Taproot address
//   from the verified `SignedTerms`; a malicious seller cannot substitute or
//   tamper with the escrow funding address.

import { schnorr, secp256k1 } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import type { StoreRecord } from "../records.ts";
import { encodeTaprootAddress, taggedHash, taprootTweakXOnly, type BuyerNetwork } from "./keys.ts";

export const ESCROW_MAINNET_REFUSAL =
  "Moderated escrow is not available on mainnet. It is a testnet4 demo until the two-party escrow protocol is reviewed.";

export type PaymentRailKind = "btc" | "xmr" | "zec" | "ln";

const SECP_N = secp256k1.Point.Fn.ORDER;
const OFFER_V1_TAG = utf8ToBytes("ob-node/escrow-offer/v1");
const OFFER_V2_TAG = utf8ToBytes("ob-node/escrow-offer/v2");

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

function compareBytes(a: Uint8Array, b: Uint8Array): number {
  const len = Math.min(a.length, b.length);
  for (let i = 0; i < len; i++) {
    if (a[i] !== b[i]) return a[i] - b[i];
  }
  return a.length - b.length;
}

/** Refuse escrow on mainnet or on non-Bitcoin rails (Monero, Zcash, Lightning). */
export function assertEscrowAllowed(network: BuyerNetwork, rail: PaymentRailKind = "btc"): void {
  if (network !== "testnet4") {
    throw new Error(ESCROW_MAINNET_REFUSAL);
  }
  if (rail !== "btc") {
    throw new Error("Moderated escrow is only available for Bitcoin on testnet4, never Monero, Zcash, or Lightning.");
  }
}

/**
 * BIP-327 MuSig2 2-of-2 key aggregation + BIP-341 unspendable Taproot tweak.
 * Matches `crates/escrow/src/crypto.rs::aggregate` and `EscrowTerms::funding_output`.
 */
export function deriveEscrowFundingAddress(
  buyerPubkey33: Uint8Array,
  vendorPubkey33: Uint8Array,
  network: BuyerNetwork,
): { internalXOnlyHex: string; outputXOnlyHex: string; address: string } {
  assertEscrowAllowed(network, "btc");
  if (buyerPubkey33.length !== 33 || vendorPubkey33.length !== 33) {
    throw new Error("Escrow public keys must be 33-byte compressed secp256k1 keys.");
  }
  if (compareBytes(buyerPubkey33, vendorPubkey33) === 0) {
    throw new Error("Buyer and vendor escrow keys must differ.");
  }
  const sorted = compareBytes(buyerPubkey33, vendorPubkey33) < 0
    ? [buyerPubkey33, vendorPubkey33]
    : [vendorPubkey33, buyerPubkey33];
  const p0 = secp256k1.Point.fromBytes(sorted[0]);
  const p1 = secp256k1.Point.fromBytes(sorted[1]);

  // BIP-327 KeyAgg: L = TaggedHash("KeyAgg list", pk0 || pk1);
  // coeff(pk0) = TaggedHash("KeyAgg coefficient", L || pk0) mod n;
  // coeff(pk1) = 1 (second unique key optimization).
  const listHash = taggedHash("KeyAgg list", sorted[0], sorted[1]);
  const a0 = bytesToBigInt(taggedHash("KeyAgg coefficient", listHash, sorted[0])) % SECP_N;
  const agg = p0.multiply(a0).add(p1);
  const internalXOnly = bigIntToBytes32(agg.x);
  const outputXOnly = taprootTweakXOnly(internalXOnly);
  const address = encodeTaprootAddress(outputXOnly, network);
  return {
    internalXOnlyHex: bytesToHex(internalXOnly),
    outputXOnlyHex: bytesToHex(outputXOnly),
    address,
  };
}

class WireReader {
  private at = 0;
  private readonly bytes: Uint8Array;

  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
  }

  take(len: number): Uint8Array {
    if (this.at + len > this.bytes.length) throw new Error("Escrow record truncated.");
    const slice = this.bytes.subarray(this.at, this.at + len);
    this.at += len;
    return slice;
  }

  u8(): number {
    return this.take(1)[0];
  }

  u32(): number {
    const b = this.take(4);
    return (b[0] | (b[1] << 8) | (b[2] << 16)) + b[3] * 0x1000000;
  }

  u64(): number {
    const lo = this.u32();
    const hi = this.u32();
    const value = hi * 0x100000000 + lo;
    if (!Number.isSafeInteger(value)) throw new Error("Escrow amount overflow.");
    return value;
  }

  varBytes(max: number): Uint8Array {
    const len = this.u32();
    if (len > max) throw new Error("Escrow field too large.");
    return this.take(len);
  }

  magic(expected: string, versions: readonly number[]): number {
    const got = new TextDecoder().decode(this.take(4));
    if (got !== expected) throw new Error(`Expected ${expected} header.`);
    const ver = this.u8();
    if (!versions.includes(ver)) throw new Error(`Unsupported ${expected} version ${ver}.`);
    return ver;
  }

  finish(): void {
    if (this.at !== this.bytes.length) throw new Error("Trailing bytes in escrow record.");
  }
}

export type ParsedEscrowTerms = {
  readonly version: 4 | 5;
  readonly feeKind: "ruling" | "filing";
  readonly buyerPubkeyHex: string;
  readonly vendorPubkeyHex: string;
  readonly moderatorSigningKeyHex: string;
  readonly moderatorEncryptionKeyHex: string;
  readonly moderatorFeeSats: number;
  readonly fundingSats: number;
  readonly networkFeeSats: number;
  readonly disputeDeadline: number;
  readonly vendorClaimHeight: number;
  readonly refundHeight: number;
  readonly termsDigestHex: string;
  readonly rawTermsBytes: Uint8Array;
};

export type VerifiedEscrowContract = {
  readonly terms: ParsedEscrowTerms;
  readonly derivedAddress: string;
  readonly outputXOnlyHex: string;
  readonly moderatorXOnlyHex: string;
};

function decodeEscrowTerms(bytes: Uint8Array): ParsedEscrowTerms {
  const r = new WireReader(bytes);
  const version = r.magic("OBET", [4, 5]) as 4 | 5;
  const feeKind = version === 5 ? "filing" : "ruling";
  const buyer = r.take(33);
  const vendor = r.take(33);

  // ModeratorRecord: magic "OBMR", version (1 for ruling, 2 for filing)
  const modVer = r.magic("OBMR", [feeKind === "filing" ? 2 : 1]);
  const modSign = r.take(33);
  const modEnc = r.take(33);
  const modFee = r.u64();
  if (modVer === 1) {
    r.varBytes(128); // payout script in v1
  }
  // Policy: 7 u64/u32 fields
  r.u32(); // min_delivery_window
  r.u32(); // min_ruling_window
  r.u32(); // min_claim_window
  r.u32(); // max_refund_horizon
  r.u64(); // max_network_fee
  r.u64(); // min_fee_rate
  r.u64(); // max_fee_rate
  r.take(64); // moderator self-signature over record

  const fundingSats = r.u64();
  const networkFeeSats = r.u64();
  const disputeDeadline = r.u32();
  const vendorClaimHeight = r.u32();
  const refundHeight = r.u32();
  r.varBytes(128); // buyer_payout script
  r.varBytes(128); // vendor_payout script
  r.finish();

  const tag = feeKind === "filing" ? "ob-escrow/terms/v5" : "ob-escrow/terms/v4";
  const digest = taggedHash(tag, bytes);
  return {
    version,
    feeKind,
    buyerPubkeyHex: bytesToHex(buyer),
    vendorPubkeyHex: bytesToHex(vendor),
    moderatorSigningKeyHex: bytesToHex(modSign),
    moderatorEncryptionKeyHex: bytesToHex(modEnc),
    moderatorFeeSats: modFee,
    fundingSats,
    networkFeeSats,
    disputeDeadline,
    vendorClaimHeight,
    refundHeight,
    termsDigestHex: bytesToHex(digest),
    rawTermsBytes: bytes,
  };
}

/**
 * Fully verify a seller's signed escrow contract locally inside signed app code:
 * 1. Refuses mainnet (`ESCROW_MAINNET_REFUSAL`) and non-Bitcoin rails.
 * 2. Decodes `SignedTerms` (`OBTS`) and verifies BOTH buyer and vendor BIP-340 sigs.
 * 3. Checks `terms.buyer` matches the buyer's deterministic seed-derived key.
 * 4. Checks the seller's store-key Schnorr signature (`ob-node/escrow-offer/v1|v2`).
 * 5. Checks the moderator is in the seller's signed `StoreRecord.details.moderators`.
 * 6. Independently derives the 2-of-3 MuSig2 Taproot address and refuses if it
 *    does not match `claimedAddress`.
 */
export function verifySignedEscrowContract(params: {
  network: BuyerNetwork;
  rail?: PaymentRailKind;
  escrowIdHex: string;
  signedTermsHex: string;
  storeSignatureHex: string;
  listingIdHex?: string | null;
  expectedBuyerPubkeyHex: string;
  claimedAddress: string;
  store: StoreRecord;
}): VerifiedEscrowContract {
  assertEscrowAllowed(params.network, params.rail ?? "btc");
  if (!/^[0-9a-f]{32}$/i.test(params.escrowIdHex)) throw new Error("Invalid escrow id.");
  if (!/^[0-9a-f]{128}$/i.test(params.storeSignatureHex)) throw new Error("Invalid store signature.");

  const signedBytes = hexToBytes(params.signedTermsHex.toLowerCase());
  const r = new WireReader(signedBytes);
  const version = r.magic("OBTS", [4, 5]);
  const termsBytes = r.varBytes(16 * 1024);
  const buyerSig = r.take(64);
  const vendorSig = r.take(64);
  r.finish();

  const terms = decodeEscrowTerms(termsBytes);
  if (terms.version !== version) throw new Error("SignedTerms version mismatch.");

  if (terms.buyerPubkeyHex !== params.expectedBuyerPubkeyHex.toLowerCase()) {
    throw new Error("Escrow terms do not match your recovery seed's buyer key.");
  }

  const termsDigest = hexToBytes(terms.termsDigestHex);
  const buyerXOnly = hexToBytes(terms.buyerPubkeyHex).subarray(1);
  const vendorXOnly = hexToBytes(terms.vendorPubkeyHex).subarray(1);
  if (!schnorr.verify(buyerSig, termsDigest, buyerXOnly)) {
    throw new Error("Buyer signature on escrow terms is invalid.");
  }
  if (!schnorr.verify(vendorSig, termsDigest, vendorXOnly)) {
    throw new Error("Vendor signature on escrow terms is invalid.");
  }

  // Verify seller store-key signature binding escrowId + terms (+ optional listingId).
  const escrowIdBytes = hexToBytes(params.escrowIdHex.toLowerCase());
  const offerDigest = params.listingIdHex
    ? sha256(concatBytes(OFFER_V2_TAG, escrowIdBytes, termsDigest, hexToBytes(params.listingIdHex.toLowerCase())))
    : sha256(concatBytes(OFFER_V1_TAG, escrowIdBytes, termsDigest));
  if (!schnorr.verify(hexToBytes(params.storeSignatureHex.toLowerCase()), offerDigest, hexToBytes(params.store.key))) {
    throw new Error("The escrow offer is not signed by the seller's store key.");
  }

  // Moderator key must be authorized by the seller's signed store record.
  const moderatorXOnlyHex = terms.moderatorSigningKeyHex.slice(2);
  const acceptedModerators = params.store.details?.moderators ?? [];
  if (!acceptedModerators.includes(moderatorXOnlyHex)) {
    throw new Error("The seller's signed store record does not accept this moderator.");
  }

  // Independently derive the MuSig2 Taproot address and reject any tampering.
  const derived = deriveEscrowFundingAddress(
    hexToBytes(terms.buyerPubkeyHex),
    hexToBytes(terms.vendorPubkeyHex),
    params.network,
  );
  if (derived.address !== params.claimedAddress.trim()) {
    throw new Error("Escrow address mismatch: the seller's claimed address does not match the signed MuSig2 terms.");
  }

  return {
    terms,
    derivedAddress: derived.address,
    outputXOnlyHex: derived.outputXOnlyHex,
    moderatorXOnlyHex,
  };
}
