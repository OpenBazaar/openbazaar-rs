// Signed catalog records, decoded and checked in the client.
//
// The node serves each store and listing as the exact bytes its store key
// signed (`record`, hex). These decoders follow crates/protocol: magic and
// version, little-endian integers, u16-length UTF-8 strings, then a 64-byte
// BIP 340 signature over the tagged hash of everything before it. A record
// that does not decode, has trailing bytes, or fails its signature is null.
import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

import type { ContentRating, ListingHeader, ListingKind } from "./catalog";

const STORE_TAG = "ob/store/v1";
const LISTING_TAG = "ob/listing/v1";
const SIG_LEN = 64;
const RATINGS: readonly ContentRating[] = ["general", "sensitive", "adult"];
const TAGS = ["nudity", "violence", "drugs", "weapons", "gambling"] as const;

export type StoreDetails = {
  mailboxKey: string;
  onion: string | null;
  moderators: string[];
  tagline: string;
  about: string;
  avatar: string | null;
  banner: string | null;
  shippingPolicy: string;
  returnsPolicy: string;
  processingTime: string;
  shipsTo: string[];
  createdOn: number;
  /** The store takes Monero direct pay. */
  acceptsXmr: boolean;
  /** The store takes Lightning direct pay. */
  acceptsLightning: boolean;
};

export type StoreRecord = {
  version: number;
  key: string;
  name: string;
  region: string;
  bolt12Offer: string;
  silentPaymentCode: string;
  contentRating: ContentRating;
  details: StoreDetails | null;
};

export type ListingRecord = {
  version: number;
  hash: string;
  storeId: string;
  title: string;
  category: string;
  priceSats: number;
  shipFrom: string;
  description: string;
  seller: string;
  stock: number;
  listedOn: number;
  moderated: boolean;
  kind: ListingKind;
  product: string;
  shippingSats: number;
  freeOverSats: number;
  contentRating: ContentRating;
  contentTags: string[];
};

export function hexToBytes(text: string): Uint8Array | null {
  if (text.length % 2 !== 0 || !/^[0-9a-f]*$/.test(text)) return null;
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function sha256Hex(bytes: Uint8Array): string {
  return bytesToHex(sha256(bytes));
}

/// BIP 340 tagged hash: SHA256(SHA256(tag) || SHA256(tag) || body).
function taggedHash(tag: string, body: Uint8Array): Uint8Array {
  const tagHash = sha256(new TextEncoder().encode(tag));
  const joined = new Uint8Array(64 + body.length);
  joined.set(tagHash, 0);
  joined.set(tagHash, 32);
  joined.set(body, 64);
  return sha256(joined);
}

class Reader {
  private at = 0;
  private readonly bytes: Uint8Array;

  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
  }

  get position(): number {
    return this.at;
  }

  take(len: number): Uint8Array {
    if (this.at + len > this.bytes.length) throw new Error("short");
    const out = this.bytes.subarray(this.at, this.at + len);
    this.at += len;
    return out;
  }

  u8(): number {
    return this.take(1)[0];
  }

  u16(): number {
    const b = this.take(2);
    return b[0] | (b[1] << 8);
  }

  u32(): number {
    const b = this.take(4);
    return (b[0] | (b[1] << 8) | (b[2] << 16)) + b[3] * 0x1000000;
  }

  /// u64 as a number; refuses values a number cannot hold exactly.
  u64(): number {
    const low = this.u32();
    const high = this.u32();
    const value = high * 0x100000000 + low;
    if (!Number.isSafeInteger(value)) throw new Error("u64 too large");
    return value;
  }

  str(): string {
    return new TextDecoder("utf-8", { fatal: true }).decode(this.take(this.u16()));
  }

  hex(len: number): string {
    return bytesToHex(this.take(len));
  }

  magic(magic: string, versions: readonly number[]): number {
    if (new TextDecoder().decode(this.take(4)) !== magic) throw new Error("magic");
    const version = this.u8();
    if (!versions.includes(version)) throw new Error("version");
    return version;
  }
}

function rating(tag: number): ContentRating {
  const value = RATINGS[tag];
  if (!value) throw new Error("rating");
  return value;
}

/// Decode `bytes` with `read`, require it to end at the signature, and check the signature.
function signed<T extends { key?: string; storeId?: string }>(
  bytes: Uint8Array,
  tag: string,
  read: (reader: Reader) => T,
): { record: T; body: Uint8Array } | null {
  try {
    const reader = new Reader(bytes);
    const record = read(reader);
    const body = bytes.subarray(0, reader.position);
    const signature = reader.take(SIG_LEN);
    if (reader.position !== bytes.length) return null;
    const key = hexToBytes(record.key ?? record.storeId ?? "");
    if (!key || !schnorr.verify(signature, taggedHash(tag, body), key)) return null;
    return { record, body };
  } catch {
    return null;
  }
}

function readStoreDetails(reader: Reader): StoreDetails {
  const mailboxKey = reader.hex(32);
  const onion = reader.str();
  const moderators = Array.from({ length: reader.u8() }, () => reader.hex(32));
  const tagline = reader.str();
  const about = reader.str();
  const image = () => {
    const flag = reader.u8();
    if (flag > 1) throw new Error("flag");
    return flag === 1 ? reader.hex(32) : null;
  };
  const avatar = image();
  const banner = image();
  const shippingPolicy = reader.str();
  const returnsPolicy = reader.str();
  const processingTime = reader.str();
  const shipsTo = Array.from({ length: reader.u8() }, () => new TextDecoder().decode(reader.take(2)));
  const createdOn = reader.u32();
  // Payment rails: bit 0 Monero, bit 1 Lightning.
  const rails = reader.u8();
  if (rails > 3) throw new Error("flag");
  const acceptsXmr = (rails & 1) === 1;
  const acceptsLightning = (rails & 2) === 2;
  return { mailboxKey, onion: onion || null, moderators, tagline, about, avatar, banner, shippingPolicy, returnsPolicy, processingTime, shipsTo, createdOn, acceptsXmr, acceptsLightning };
}

/// A store record (versions 1 to 3) whose signature checks against its own key.
export function verifyStoreRecord(hex: unknown): StoreRecord | null {
  const bytes = typeof hex === "string" ? hexToBytes(hex) : null;
  if (!bytes) return null;
  const result = signed(bytes, STORE_TAG, (reader) => {
    const version = reader.magic("OBST", [1, 2, 3]);
    const name = reader.str();
    const region = reader.str();
    const bolt12Offer = reader.str();
    const silentPaymentCode = reader.str();
    const key = reader.hex(32);
    const contentRating = version >= 2 ? rating(reader.u8()) : "unrated";
    const details = version === 3 ? readStoreDetails(reader) : null;
    return { version, key, name, region, bolt12Offer, silentPaymentCode, contentRating, details };
  });
  return result?.record ?? null;
}

/// A listing record (versions 4 and 5) whose signature checks against its store key.
/// `hash` is SHA-256 of the signed body: the listing's address.
export function verifyListingRecord(hex: unknown): ListingRecord | null {
  const bytes = typeof hex === "string" ? hexToBytes(hex) : null;
  if (!bytes) return null;
  const result = signed(bytes, LISTING_TAG, (reader) => {
    const version = reader.magic("OBLS", [4, 5]);
    const storeId = reader.hex(32);
    const title = reader.str();
    const category = reader.str();
    const priceSats = reader.u64();
    const shipFrom = reader.str();
    const description = reader.str();
    const seller = reader.str();
    const stock = reader.u32();
    const listedOn = reader.u32();
    const moderated = reader.u8() === 1;
    const kindTag = reader.u8();
    if (kindTag > 1) throw new Error("kind");
    const kind: ListingKind = kindTag === 0 ? "physical" : "digital";
    const product = reader.str();
    const shippingSats = reader.u64();
    const freeOverSats = reader.u64();
    let contentRating: ContentRating = "unrated";
    let contentTags: string[] = [];
    if (version === 5) {
      contentRating = rating(reader.u8());
      const bits = reader.u16();
      contentTags = TAGS.filter((_, i) => bits & (1 << i));
    }
    return { version, hash: "", storeId, title, category, priceSats, shipFrom, description, seller, stock, listedOn, moderated, kind, product, shippingSats, freeOverSats, contentRating, contentTags };
  });
  if (!result) return null;
  return { ...result.record, hash: sha256Hex(result.body) };
}

/// One store in a store list, from its verified record. `listingCount` and
/// `own` are the node's bookkeeping, not signed.
export type StoreSummary = {
  key: string;
  name: string;
  region: string;
  tagline: string;
  avatar: string | null;
  contentRating: ContentRating;
  /** From the signed record: the store takes Monero direct pay. */
  acceptsXmr: boolean;
  /** From the signed record: the store takes Lightning direct pay. */
  acceptsLightning: boolean;
  listingCount: number;
  own: boolean;
  version: number;
  record: StoreRecord;
};

function recordOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" ? (value as Record<string, unknown>) : null;
}

/// A store summary from the node, or null when its record does not verify.
export function verifiedStore(value: unknown): StoreSummary | null {
  const item = recordOf(value);
  const record = item ? verifyStoreRecord(item.record) : null;
  if (!item || !record || item.key !== record.key) return null;
  return {
    key: record.key,
    name: record.name,
    region: record.region,
    tagline: record.details?.tagline ?? "",
    avatar: record.details?.avatar ?? null,
    contentRating: record.contentRating,
    acceptsXmr: record.details?.acceptsXmr ?? false,
    acceptsLightning: record.details?.acceptsLightning ?? false,
    listingCount: typeof item.listingCount === "number" ? item.listingCount : 0,
    own: item.own === true,
    version: record.version,
    record,
  };
}

/// A listing header built from its verified record, or null when the record
/// fails, its hash is not the one the node named, or its store is not in `storeKeys`.
/// `acceptsModerator` is the node's check against its moderator, and only
/// stands if the signed record says the listing takes escrow. `acceptsXmr` is
/// the node's (Monero is on) and only stands if the store is in `xmrStores`,
/// the stores whose signed record takes Monero. `acceptsLightning` likewise
/// with `lnStores`.
export function verifiedListing(
  value: unknown,
  storeKeys: ReadonlySet<string>,
  xmrStores: ReadonlySet<string> = new Set(),
  lnStores: ReadonlySet<string> = new Set(),
): ListingHeader | null {
  const item = recordOf(value);
  const record = item ? verifyListingRecord(item.record) : null;
  if (!item || !record || item.hash !== record.hash || !storeKeys.has(record.storeId)) return null;
  const description = record.description.trim();
  return {
    hash: record.hash,
    storeId: record.storeId,
    title: record.title,
    category: record.category,
    priceSats: record.priceSats,
    shipFrom: record.shipFrom,
    kind: record.kind,
    acceptsModerator: item.acceptsModerator === true && record.moderated,
    acceptsXmr: item.acceptsXmr === true && xmrStores.has(record.storeId),
    acceptsLightning: item.acceptsLightning === true && lnStores.has(record.storeId),
    description: description || undefined,
    seller: record.seller,
    stock: record.stock,
    listedOn: record.listedOn,
    product: record.product,
    shippingSats: record.shippingSats,
    freeOverSats: record.freeOverSats,
    contentRating: record.contentRating,
    contentTags: record.contentTags,
  };
}

/// Every listing that verifies, and how many did not.
export function verifiedListings(
  values: unknown,
  storeKeys: ReadonlySet<string>,
  xmrStores: ReadonlySet<string> = new Set(),
  lnStores: ReadonlySet<string> = new Set(),
): { listings: ListingHeader[]; rejected: number } {
  const items = Array.isArray(values) ? values : [];
  const listings = items.map((item) => verifiedListing(item, storeKeys, xmrStores, lnStores)).filter((item): item is ListingHeader => item !== null);
  return { listings, rejected: items.length - listings.length };
}
