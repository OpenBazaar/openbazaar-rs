// Version 7 listing terms and the small signed records around a listing,
// read from the signed bytes the way crates/protocol writes them.
//
// The shapes match what the node returns for a listing's "commerce" and what
// the editor sends, so one type serves the buyer and the seller. Amounts are
// minor units of the listing's currency. Days are YYYYMMDD numbers.

import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

export type Condition = "new" | "open-box" | "like-new" | "good" | "fair" | "for-parts";
export const CONDITIONS: readonly Condition[] = ["new", "open-box", "like-new", "good", "fair", "for-parts"];
export const CONDITION_LABELS: Record<Condition, string> = { new: "New", "open-box": "Open box", "like-new": "Like new", good: "Good", fair: "Fair", "for-parts": "For parts or not working" };

export type Axis = { name: string; values: string[] };
export type Variant = { id: string; options: string[]; sku: string; price: number; stock: number; image: number; weightG: number };
export type Service = { name: string; price: number; extraItem: number; daysMin: number; daysMax: number; regions: string[] };
export type Availability = { kind: "in-stock" } | { kind: "pre-order"; shipsOn: number } | { kind: "made-to-order"; days: number } | { kind: "away" };
export type Returns = { kind: "store" } | { kind: "none" } | { kind: "days"; days: number; sellerPays: boolean };
export type Sale = { offBps: number; startsOn: number; endsOn: number };
export type Tier = { minQty: number; offBps: number };

export type Commerce = {
  condition: Condition;
  conditionNote: string;
  taxonomy: number;
  specifics: Record<string, string>;
  axes: Axis[];
  variants: Variant[];
  compareAt: number;
  sale: Sale | null;
  tiers: Tier[];
  acceptsOffers: boolean;
  maxPerOrder: number;
  lotSize: number;
  availability: Availability;
  shipping: { weightG: number; dimsMm: [number, number, number]; handlingDays: number; freeOver: number; services: Service[]; pickup: string | null };
  returns: Returns;
};

export type FiatPrice = { currency: string; amount: number; shipping: number };

/// The parts of a listing record that price an order.
export type ListingTerms = {
  version: number;
  storeId: string;
  kind: "physical" | "digital";
  stock: number;
  listingId: string | null;
  images: string[];
  price: FiatPrice | null;
  commerce: Commerce | null;
};

const decoder = new TextDecoder("utf-8", { fatal: true });

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
}

export function fromHex(hex: string): Uint8Array {
  if (!/^(?:[0-9a-f]{2})*$/.test(hex)) throw new Error("hex");
  return Uint8Array.from(hex.match(/../g) ?? [], byte => parseInt(byte, 16));
}

export class ByteReader {
  at = 0;
  readonly bytes: Uint8Array;
  constructor(bytes: Uint8Array) { this.bytes = bytes; }
  take(n: number): Uint8Array {
    if (this.at + n > this.bytes.length) throw new Error("truncated");
    const out = this.bytes.subarray(this.at, this.at + n);
    this.at += n;
    return out;
  }
  u8() { return this.take(1)[0]; }
  u16() { const b = this.take(2); return b[0] | (b[1] << 8); }
  u32() { const b = this.take(4); return (b[0] | (b[1] << 8) | (b[2] << 16)) + b[3] * 2 ** 24; }
  u64() {
    const value = this.u32() + this.u32() * 2 ** 32;
    if (!Number.isSafeInteger(value)) throw new Error("amount too large");
    return value;
  }
  hex(n: number) { return toHex(this.take(n)); }
  str() { return decoder.decode(this.take(this.u16())); }
  blob() { return this.take(this.u32()); }
  magic(magic: string, versions: readonly number[]): number {
    if (decoder.decode(this.take(4)) !== magic) throw new Error("magic");
    const version = this.u8();
    if (!versions.includes(version)) throw new Error("version");
    return version;
  }
  done() { if (this.at !== this.bytes.length) throw new Error("trailing bytes"); }
}

function times<T>(n: number, read: () => T): T[] {
  return Array.from({ length: n }, read);
}

/// What `readCommerce` needs of a reader; `records.ts` passes its own.
export type FieldReader = Pick<ByteReader, "take" | "u8" | "u16" | "u32" | "u64" | "hex" | "str">;

/// The commerce block of a version 7 listing.
export function readCommerce(r: FieldReader): Commerce {
  const condition = CONDITIONS[r.u8()];
  if (!condition) throw new Error("condition");
  const conditionNote = r.str();
  const taxonomy = r.u16();
  const specifics = Object.fromEntries(times(r.u8(), () => [r.str(), r.str()] as const));
  const axes = times(r.u8(), () => ({ name: r.str(), values: times(r.u8(), () => r.str()) }));
  const variants = times(r.u8(), () => {
    const id = r.hex(8);
    const choice = axes.map(() => r.u8());
    const options = choice.map((index, axis) => {
      const value = axes[axis].values[index];
      if (value === undefined) throw new Error("variant choice");
      return value;
    });
    return { id, options, sku: r.str(), price: r.u64(), stock: r.u32(), image: r.u8(), weightG: r.u32() };
  });
  const compareAt = r.u64();
  const sale = r.u8() === 1 ? { offBps: r.u16(), startsOn: r.u32(), endsOn: r.u32() } : null;
  const tiers = times(r.u8(), () => ({ minQty: r.u16(), offBps: r.u16() }));
  const acceptsOffers = r.u8() === 1;
  const maxPerOrder = r.u16();
  const lotSize = r.u16();
  const tag = r.u8();
  const value = r.u32();
  const availability: Availability = tag === 1 ? { kind: "pre-order", shipsOn: value } : tag === 2 ? { kind: "made-to-order", days: value } : tag === 3 ? { kind: "away" } : { kind: "in-stock" };
  const weightG = r.u32();
  const dimsMm: [number, number, number] = [r.u16(), r.u16(), r.u16()];
  const handlingDays = r.u8();
  const freeOver = r.u64();
  const services = times(r.u8(), () => ({
    name: r.str(), price: r.u64(), extraItem: r.u64(), daysMin: r.u8(), daysMax: r.u8(),
    regions: times(r.u8(), () => decoder.decode(r.take(2))),
  }));
  const pickup = r.u8() === 1 ? r.str() : null;
  const returnsTag = r.u8();
  const days = r.u16();
  const returns: Returns = returnsTag === 1 ? { kind: "none" } : returnsTag >= 2 ? { kind: "days", days, sellerPays: returnsTag === 3 } : { kind: "store" };
  return {
    condition, conditionNote, taxonomy, specifics, axes, variants, compareAt, sale, tiers, acceptsOffers, maxPerOrder, lotSize, availability,
    shipping: { weightG, dimsMm, handlingDays, freeOver, services, pickup }, returns,
  };
}

/// Read a listing record of version 4 to 7 for its terms. This does not
/// check the signature: `records.ts` does, and drops a listing that fails.
export function decodeListingTerms(bytes: Uint8Array): ListingTerms {
  const r = new ByteReader(bytes);
  const version = r.magic("OBLS", [4, 5, 6, 7]);
  const storeId = r.hex(32);
  r.str(); r.str(); r.u64(); r.str(); r.str(); r.str();
  const stock = r.u32();
  r.u32(); r.u8();
  const kind = r.u8() === 1 ? "digital" : "physical";
  r.str(); r.u64(); r.u64();
  if (version >= 5) { r.u8(); r.u16(); }
  let listingId: string | null = null;
  let images: string[] = [];
  let price: FiatPrice | null = null;
  if (version >= 6) {
    listingId = r.hex(16);
    images = times(r.u8(), () => r.hex(32));
    price = r.u8() === 1 ? { currency: r.str(), amount: r.u64(), shipping: r.u64() } : null;
  }
  const commerce = version === 7 ? readCommerce(r) : null;
  r.take(64);
  r.done();
  return { version, storeId, kind, stock, listingId, images, price, commerce };
}

function taggedHash(tag: string, body: Uint8Array): Uint8Array {
  const tagHash = sha256(new TextEncoder().encode(tag));
  const joined = new Uint8Array(64 + body.length);
  joined.set(tagHash, 0);
  joined.set(tagHash, 32);
  joined.set(body, 64);
  return sha256(joined);
}

/// BIP 340 over the tagged hash of everything before the 64-byte signature.
function verifyTail(tag: string, bytes: Uint8Array, key: string): boolean {
  if (bytes.length < 64) return false;
  try {
    return schnorr.verify(bytes.subarray(bytes.length - 64), taggedHash(tag, bytes.subarray(0, bytes.length - 64)), fromHex(key));
  } catch {
    return false;
  }
}

export type Coupon = {
  record: string;
  storeId: string;
  code: string;
  discount: { kind: "percent"; bps: number } | { kind: "fixed"; amount: number };
  currency: string;
  minSubtotal: number;
  listings: string[];
  startsOn: number;
  endsOn: number;
  maxUses: number;
};

/// A coupon record whose store signature checks, or null.
export function verifyCoupon(hex: string): Coupon | null {
  try {
    const bytes = fromHex(hex);
    const r = new ByteReader(bytes);
    r.magic("OBCP", [1]);
    const storeId = r.hex(32);
    const code = r.str();
    const kind = r.u8();
    const amount = r.u64();
    const discount = kind === 0 ? { kind: "percent" as const, bps: amount } : { kind: "fixed" as const, amount };
    const coupon = { record: hex, storeId, code, discount, currency: r.str(), minSubtotal: r.u64(), listings: times(r.u8(), () => r.hex(16)), startsOn: r.u32(), endsOn: r.u32(), maxUses: r.u32() };
    r.take(64);
    r.done();
    return verifyTail("ob/coupon/v1", bytes, storeId) ? coupon : null;
  } catch {
    return null;
  }
}

export type Offer = {
  record: string;
  storeId: string;
  offerId: string;
  listingId: string;
  variantId: string;
  quantity: number;
  amount: number;
  currency: string;
  buyerKey: string;
  expiresAt: number;
  counter: boolean;
};

/// An offer record whose store signature checks, or null.
export function verifyOffer(hex: string): Offer | null {
  try {
    const bytes = fromHex(hex);
    const r = new ByteReader(bytes);
    r.magic("OBOF", [1]);
    const offer = {
      record: hex, storeId: r.hex(32), offerId: r.hex(16), listingId: r.hex(16), variantId: r.hex(8), quantity: r.u16(),
      amount: r.u64(), currency: r.str(), buyerKey: r.hex(32), expiresAt: r.u64(), counter: r.u8() === 1,
    };
    r.take(64);
    r.done();
    return verifyTail("ob/offer/v1", bytes, offer.storeId) ? offer : null;
  } catch {
    return null;
  }
}

export type Review = { storeId: string; listingId: string; reviewKey: string; paidOn: number; rating: number; text: string; writtenOn: number };

/// A review whose receipt is signed by its store and whose body is signed by
/// the receipt's review key, or null.
export function verifyReview(hex: string): Review | null {
  try {
    const bytes = fromHex(hex);
    const r = new ByteReader(bytes);
    r.magic("OBRV", [1]);
    const receiptBytes = r.blob();
    const rating = r.u8();
    const text = r.str();
    const writtenOn = r.u32();
    r.take(64);
    r.done();
    const receipt = new ByteReader(receiptBytes);
    receipt.magic("OBRC", [1]);
    const storeId = receipt.hex(32);
    const listingId = receipt.hex(16);
    const reviewKey = receipt.hex(32);
    const paidOn = receipt.u32();
    receipt.take(64);
    receipt.done();
    if (rating < 1 || rating > 5 || writtenOn < paidOn) return null;
    if (!verifyTail("ob/receipt/v1", receiptBytes, storeId) || !verifyTail("ob/review/v1", bytes, reviewKey)) return null;
    return { storeId, listingId, reviewKey, paidOn, rating, text, writtenOn };
  } catch {
    return null;
  }
}
