import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

import { parseCatalog, storeRatingOf } from "./catalog.ts";
import { bytesToHex, hexToBytes, verifiedListing, verifyDisputeDecision, verifyFeeReceipt, verifyListingRecord, verifyStoreRecord } from "./records.ts";

type Raw = { stores: Record<string, unknown>[]; listings: Record<string, unknown>[] };
const raw = (): Raw => JSON.parse(readFileSync(new URL("./catalog.json", import.meta.url), "utf8")) as Raw;

function flip(hex: string, at: number): string {
  const bytes = hexToBytes(hex)!;
  bytes[at] ^= 1;
  return bytesToHex(bytes);
}

test("every demo store and listing record verifies, as signed by the node", () => {
  const { stores, listings } = raw();
  assert.equal(stores.length, 3);
  for (const store of stores) {
    const record = verifyStoreRecord(store.record);
    assert.ok(record, String(store.name));
    assert.equal(record.key, store.key);
    assert.equal(record.name, store.name);
    assert.equal(record.version, 3);
    assert.equal(record.details?.tagline, store.tagline);
    assert.equal(record.details?.avatar, store.avatar);
    assert.ok(record.details?.mailboxKey.length === 64);
    assert.ok(record.bolt12Offer.startsWith("lno1"));
  }
  for (const listing of listings) {
    const record = verifyListingRecord(listing.record);
    assert.ok(record, String(listing.title));
    assert.equal(record.hash, listing.hash);
    for (const key of ["storeId", "title", "category", "priceSats", "shipFrom", "seller", "stock", "listedOn", "kind", "product", "shippingSats", "freeOverSats", "contentRating"] as const) {
      assert.deepEqual(record[key], listing[key], `${String(listing.title)} ${key}`);
    }
    assert.deepEqual(record.contentTags, listing.contentTags);
  }
});

test("a sample store flag from the node is kept on the card", () => {
  const { listings } = raw();
  const item = { ...listings[0], sample: true };
  const record = verifyListingRecord(item.record);
  assert.ok(record);
  const header = verifiedListing(item, new Set([record.storeId]));
  assert.equal(header?.sample, true);
  assert.equal(verifiedListing({ ...listings[0], sample: false }, new Set([record.storeId]))?.sample, false);
});

test("a changed byte, a missing record, or trailing bytes fail", () => {
  const { stores, listings } = raw();
  const store = String(stores[0].record);
  const listing = String(listings[0].record);
  for (const at of [0, 5, 10, store.length / 2 - 1]) assert.equal(verifyStoreRecord(flip(store, at)), null, `store byte ${at}`);
  for (const at of [0, 5, 40, listing.length / 2 - 1]) assert.equal(verifyListingRecord(flip(listing, at)), null, `listing byte ${at}`);
  assert.equal(verifyStoreRecord(undefined), null);
  assert.equal(verifyStoreRecord(store + "00"), null);
  assert.equal(verifyStoreRecord(store.toUpperCase()), null);
  assert.equal(verifyListingRecord(store), null, "a store record is not a listing");
});

test("a record signed by another key does not verify", () => {
  const { listings } = raw();
  const bytes = hexToBytes(String(listings[0].record))!;
  const secret = schnorr.utils.randomSecretKey();
  const body = bytes.subarray(0, bytes.length - 64);
  const forged = new Uint8Array(bytes);
  forged.set(schnorr.sign(new Uint8Array(32), secret), body.length);
  assert.equal(verifyListingRecord(bytesToHex(forged)), null);
});

test("the catalog keeps only verified listings from verified stores", () => {
  const catalog = parseCatalog(raw());
  assert.equal(catalog.stores.length, 3);
  assert.equal(catalog.rejected, 0);
  assert.equal(catalog.listings.length, raw().listings.length);
  assert.equal(catalog.store.name, "Marlow Goods");
  assert.equal(catalog.store.contentRating, "general");
  assert.equal(storeRatingOf(catalog, catalog.listings[0]), "general");

  const tampered = raw();
  tampered.listings[0].record = flip(String(tampered.listings[0].record), 20);
  tampered.listings[1].hash = "00".repeat(32);
  delete tampered.listings[2].record;
  tampered.listings[3].title = "Not what was signed";
  const checked = parseCatalog(tampered);
  assert.equal(checked.rejected, 3);
  assert.equal(checked.listings.length, raw().listings.length - 3);
  assert.equal(checked.listings.find((item) => item.hash === tampered.listings[3].hash)?.title, raw().listings[3].title, "headers come from the signed record");

  const orphaned = raw();
  orphaned.stores = orphaned.stores.slice(1);
  const marlow = String(raw().stores[0].key);
  const result = parseCatalog(orphaned);
  assert.ok(result.listings.every((item) => item.storeId !== marlow));
  assert.equal(result.rejected, raw().listings.filter((item) => item.storeId === marlow).length);
});

test("escrow stands only if the signed listing takes it", () => {
  const listing = { ...raw().listings[0], acceptsModerator: true };
  const keys = new Set([String(listing.storeId)]);
  const record = verifyListingRecord(listing.record)!;
  assert.equal(verifiedListing(listing, keys)?.acceptsModerator, record.moderated);
  assert.equal(verifiedListing({ ...listing, acceptsModerator: false }, keys)?.acceptsModerator, false);
  assert.equal(verifiedListing(listing, new Set()), null);
});

test("Monero stands only if the store's signed record takes it", () => {
  const store = String(raw().stores[0].record);
  assert.equal(verifyStoreRecord(store)?.details?.acceptsXmr, false);
  const last = store.length / 2 - 64 - 1;
  const bytes = hexToBytes(store)!;
  assert.equal(bytes[last], 0, "the rail flags are the last signed byte");
  bytes[last] = 8;
  assert.equal(verifyStoreRecord(bytesToHex(bytes)), null, "an unknown rail flag does not decode");
  assert.equal(verifyStoreRecord(flip(store, last)), null, "the flag is signed");

  const listing = { ...raw().listings[0], acceptsXmr: true };
  const key = String(listing.storeId);
  assert.equal(verifiedListing(listing, new Set([key]))?.acceptsXmr, false, "the node's word alone is not enough");
  assert.equal(verifiedListing(listing, new Set([key]), new Set([key]))?.acceptsXmr, true);
  assert.equal(verifiedListing({ ...listing, acceptsXmr: false }, new Set([key]), new Set([key]))?.acceptsXmr, false);
});

test("the Zcash flag is read from the signed record, beside the other rails", () => {
  const zcash = verifyStoreRecord(resignedStore(4).hex)?.details;
  assert.deepEqual([zcash?.acceptsXmr, zcash?.acceptsLightning, zcash?.acceptsZec], [false, false, true]);
  const all = verifyStoreRecord(resignedStore(7).hex)?.details;
  assert.deepEqual([all?.acceptsXmr, all?.acceptsLightning, all?.acceptsZec], [true, true, true]);
  assert.equal(verifyStoreRecord(resignedStore(3).hex)?.details?.acceptsZec, false, "a record signed before Zcash reads the same");
  assert.equal(verifyStoreRecord(raw().stores[0].record)?.details?.acceptsZec, false);
  const store = resignedStore(4).hex;
  assert.equal(verifyStoreRecord(flip(store, store.length / 2 - 64 - 1)), null, "the flag is signed");

  const listing = { ...raw().listings[0], acceptsZec: true };
  const key = String(listing.storeId);
  const zecStores = new Set([key]);
  assert.equal(verifiedListing(listing, new Set([key]))?.acceptsZec, false, "the node's word alone is not enough");
  assert.equal(verifiedListing(listing, new Set([key]), new Set(), new Set(), zecStores)?.acceptsZec, true);
  assert.equal(verifiedListing({ ...listing, acceptsZec: false }, new Set([key]), new Set(), new Set(), zecStores)?.acceptsZec, false, "the store signed it, but Zcash is off on the node");
  assert.equal(verifiedListing(listing, new Set([key]), new Set([key]), new Set([key]))?.acceptsZec, false, "Monero and Lightning are not Zcash");
});

/// The demo store record with the rail flags set to `rails`, signed again by a new key.
function resignedStore(rails: number): { hex: string; key: string } {
  const store = raw().stores[0];
  const bytes = hexToBytes(String(store.record))!;
  const record = verifyStoreRecord(store.record)!;
  const utf8 = (text: string) => new TextEncoder().encode(text).length + 2;
  const keyAt = 5 + utf8(record.name) + utf8(record.region) + utf8(record.bolt12Offer) + utf8(record.silentPaymentCode);
  const secret = schnorr.utils.randomSecretKey();
  const key = schnorr.getPublicKey(secret);
  const body = bytes.slice(0, bytes.length - 64);
  body.set(key, keyAt);
  body[body.length - 1] = rails;
  const tag = sha256(new TextEncoder().encode("ob/store/v1"));
  const signature = schnorr.sign(sha256(new Uint8Array([...tag, ...tag, ...body])), secret);
  return { hex: bytesToHex(new Uint8Array([...body, ...signature])), key: bytesToHex(key) };
}

test("Lightning stands only if the store's signed record takes it", () => {
  const both = resignedStore(3);
  const details = verifyStoreRecord(both.hex)?.details;
  assert.equal(details?.acceptsXmr, true);
  assert.equal(details?.acceptsLightning, true);
  const lightning = verifyStoreRecord(resignedStore(2).hex)?.details;
  assert.deepEqual([lightning?.acceptsXmr, lightning?.acceptsLightning], [false, true]);
  assert.equal(verifyStoreRecord(resignedStore(8).hex), null, "an unknown rail flag does not decode");
  assert.equal(verifyStoreRecord(raw().stores[0].record)?.details?.acceptsLightning, false);

  const listing = { ...raw().listings[0], acceptsLightning: true };
  const key = String(listing.storeId);
  assert.equal(verifiedListing(listing, new Set([key]))?.acceptsLightning, false, "the node's word alone is not enough");
  assert.equal(verifiedListing(listing, new Set([key]), new Set(), new Set([key]))?.acceptsLightning, true);
  assert.equal(verifiedListing(listing, new Set([key]), new Set([key]))?.acceptsLightning, false, "Monero is not Lightning");
});

test("a catalog from a node that refuses Lightning names why", () => {
  const blocked = "Lightning on mainnet is disabled until its connections run over Tor.";
  const catalog = parseCatalog({ ...raw(), lightning: { enabled: false, running: false, network: null, blocked } });
  assert.equal(catalog.lightningBlocked, blocked);
  assert.ok(catalog.listings.every((item) => !item.acceptsLightning));
  assert.equal(parseCatalog(raw()).lightningBlocked, null);
});

// Signed by crates/protocol with the secret [7; 32].
const MODERATOR = "989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f";
const ON_CHAIN_RECEIPT = "4f42465201989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f1111111111111111111111111111111111111111111111111111111111111111222222222222222222222222222222222222222222222222222222222222222202333333333333333333333333333333333333333333333333333333333333333301000000c409000000000000780000002c010000ccf956344b339203b6e19a51cb854c1efdbc914a8d94214b60e23b45951e99125da536c4225726aa546e8caf510f89e82b798c9ed46b008d6ace26154158b7d2";
const LN_RECEIPT = "4f42465201989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f11111111111111111111111111111111111111111111111111111111111111112222222222222222222222222222222222222222222222222222222222222222014444444444444444444444444444444444444444444444444444444444444444c409000000000000780000002c0100008834a2eceb5b6aa924f38434d6d49b74aee0309b5e33e72c1baebe2ea192e66906eb559d74965c81a9ce6b67ba4dd500217f0c715aaf7aa889e8ebfdd8a5ca4c";
const DENIAL = "4f42444401989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f11111111111111111111111111111111111111111111111111111111111111110300000000000000000000000000000000000000000000000000000000000000005e2f1a15d12c2c4e53d01057269ba75c0b8a7ea98556f7991e542331ee990872dbf724ebbdb39a5279094af1e30d89571f940684ba513f7e013f4ca1f91ec64b";

test("a filing fee receipt verifies against the moderator's key, as the node signs it", () => {
  const receipt = verifyFeeReceipt(ON_CHAIN_RECEIPT, MODERATOR);
  assert.deepEqual(receipt, {
    key: MODERATOR,
    caseId: "11".repeat(32),
    requestDigest: "22".repeat(32),
    payment: { rail: "onchain", txid: "33".repeat(32), vout: 1 },
    receivedSats: 2500,
    paidHeight: 120,
    dueHeight: 300,
  });
  assert.deepEqual(verifyFeeReceipt(LN_RECEIPT, MODERATOR)?.payment, { rail: "ln", paymentHash: "44".repeat(32) });
  assert.equal(verifyFeeReceipt(ON_CHAIN_RECEIPT, "8f".repeat(32)), null, "another moderator's key");
  assert.equal(verifyFeeReceipt(flip(ON_CHAIN_RECEIPT, 140), MODERATOR), null, "a changed amount");
  assert.equal(verifyFeeReceipt(DENIAL, MODERATOR), null, "a decision is not a receipt");
  assert.equal(verifyFeeReceipt(`${ON_CHAIN_RECEIPT}00`, MODERATOR), null);
});

test("a denial verifies as a final decision on the buyer's letter", () => {
  assert.deepEqual(verifyDisputeDecision(DENIAL, MODERATOR), { key: MODERATOR, caseId: "11".repeat(32), decision: "denied-buyer", rulingDigest: "00".repeat(32) });
  assert.equal(verifyDisputeDecision(flip(DENIAL, 70), MODERATOR), null);
  assert.equal(verifyDisputeDecision(LN_RECEIPT, MODERATOR), null);
});
