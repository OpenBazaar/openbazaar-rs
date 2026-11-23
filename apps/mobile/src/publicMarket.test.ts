import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

import { verifyAdmissionIndex, verifyCatalogIndex } from "./catalogIndex.ts";
import { admittedAddresses, aggregatePublicStores, marketNotice, NODE_REQUIRED, nodeRequired } from "./publicMarket.ts";
import { bytesToHex, hexToBytes, verifyListingRecord, verifyStoreRecord } from "./records.ts";

type Raw = { stores: { key: string; name: string; record: string }[]; listings: { storeId: string; record: string; title: string }[] };
const raw = (): Raw => JSON.parse(readFileSync(new URL("./catalog.json", import.meta.url), "utf8")) as Raw;

const LABELS: Record<string, string> = {
  "Marlow Goods": "openbazaar/phase0/fixture/store",
  "Hale Paper": "openbazaar/testnet4/fixture/store/hale-paper",
  "North Kitchen": "openbazaar/testnet4/fixture/store/north-kitchen",
};

function secretFor(name: string): Uint8Array {
  return sha256(new TextEncoder().encode(LABELS[name]));
}

function u16(value: number): Uint8Array {
  return Uint8Array.of(value & 255, value >> 8);
}
function u32(value: number): Uint8Array {
  return Uint8Array.of(value & 255, (value >> 8) & 255, (value >> 16) & 255, (value >>> 24) & 255);
}
function u64(value: number): Uint8Array {
  const out = new Uint8Array(8);
  let left = value;
  for (let i = 0; i < 8; i++) {
    out[i] = left & 255;
    left = Math.floor(left / 256);
  }
  return out;
}
function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}
function tagged(tag: string, body: Uint8Array): Uint8Array {
  const hash = sha256(new TextEncoder().encode(tag));
  return sha256(concat([hash, hash, body]));
}

function signCatalog(name: string, recordHex: string, hashes: string[]): string {
  const secret = secretFor(name);
  const key = schnorr.getPublicKey(secret);
  const ids = hashes.map((hash) => hexToBytes(hash)!).sort((a, b) => {
    for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return a[i] - b[i];
    return 0;
  });
  const body = concat([
    new TextEncoder().encode("OBCX"),
    Uint8Array.of(1),
    key,
    u64(1),
    u32(20261123),
    sha256(hexToBytes(recordHex)!),
    u16(ids.length),
    ...ids,
  ]);
  return bytesToHex(concat([body, schnorr.sign(tagged("ob/catalog-index/v1", body), secret)]));
}

function storeBody(name: string, listings: Raw["listings"], index: string[] | null) {
  const store = raw().stores.find((item) => item.name === name)!;
  const body: { store: string; listings: string[]; catalogIndex?: string } = {
    store: store.record,
    listings: listings.filter((item) => item.storeId === store.key).map((item) => item.record),
  };
  if (index) {
    const hashes = index.map((title) => verifyListingRecord(listings.find((item) => item.title === title)?.record)!.hash);
    body.catalogIndex = signCatalog(name, store.record, hashes);
  }
  return body;
}

test("signed listings from fixture stores aggregate into one market", () => {
  const { stores, listings } = raw();
  for (const store of stores) {
    const key = bytesToHex(schnorr.getPublicKey(secretFor(store.name)));
    assert.equal(verifyStoreRecord(store.record)?.key, key, store.name);
  }
  const market = aggregatePublicStores([storeBody("Marlow Goods", listings, null), storeBody("Hale Paper", listings, null)]);
  const marlow = stores.find((store) => store.name === "Marlow Goods")!;
  const hale = stores.find((store) => store.name === "Hale Paper")!;
  const expected = listings.filter((item) => item.storeId === marlow.key || item.storeId === hale.key);
  assert.equal(market.listings.length, expected.length);
  assert.equal(market.stores.map((store) => store.name).sort().join(","), "Hale Paper,Marlow Goods");
  assert.equal(market.rejected, 0);
  assert.ok(market.listings.some((item) => item.seller === "Marlow Goods"));
  assert.ok(market.listings.some((item) => item.seller === "Hale Paper"));
});

test("a bad signature and an index miss are dropped", () => {
  const { listings } = raw();
  const hale = listings.filter((item) => item.seller === "Hale Paper");
  assert.ok(hale.length >= 2);
  const kept = hale[0].title;
  const missed = hale[1].title;
  const body = storeBody("Hale Paper", listings, [kept]);
  const flipped = `${hale[2].record.slice(0, 20)}00${hale[2].record.slice(22)}`;
  body.listings = [body.listings.find((record) => verifyListingRecord(record)?.title === kept)!, hale[1].record, flipped];
  const market = aggregatePublicStores([body]);
  assert.deepEqual(market.listings.map((item) => item.title), [kept]);
  assert.equal(market.rejected, 2, "the index miss and the bad signature");
  assert.equal(verifyCatalogIndex("00", market.stores[0].key, body.store), null);
  assert.equal(verifyCatalogIndex(`${body.catalogIndex!.slice(0, -2)}00`, market.stores[0].key, body.store), null);
});

test("buying and selling stay on the node's download", () => {
  const mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";
  const buy = nodeRequired("buy", mac);
  const sell = nodeRequired("sell", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
  assert.equal(buy.text, NODE_REQUIRED);
  assert.equal(sell.text, NODE_REQUIRED);
  assert.match(buy.text, /Buying and selling run on your own node/);
  assert.equal(buy.href, "https://github.com/OpenBazaar/openbazaar-rs/releases/download/v0.1.0-alpha.14/OpenBazaar_0.1.0-alpha.14_universal.dmg");
  assert.equal(sell.href, "https://github.com/OpenBazaar/openbazaar-rs/releases/download/v0.1.0-alpha.14/OpenBazaar_0.1.0-alpha.14_x64-setup.exe");
  assert.equal(buy.title, "Buying runs on your node");
  assert.equal(sell.title, "Selling runs on your node");
  const onion = "yc546t4ecasnoollz3voqrrqsevup3wk25g4d7mxaryfomtu7xh37ead.onion";
  const https = marketNotice("https:", "moderator.example", onion);
  assert.equal(https?.sentence, `This page is https, so it cannot load onion stores; open the market in Tor Browser at http://${onion}/.`);
  assert.equal(marketNotice("http:", onion, onion), null);
  assert.deepEqual(admittedAddresses(["aa".repeat(32), "bb".repeat(32)], [
    { storeId: "aa".repeat(32), url: `http://${"c".repeat(56)}.onion` },
    { storeId: "cc".repeat(32), url: `http://${"d".repeat(56)}.onion` },
    { storeId: "bb".repeat(32), url: "http://shop.example" },
  ]).map((row) => row.storeId), ["aa".repeat(32)]);
});

test("an admission snapshot keeps only store ids the directory key signed", () => {
  const secret = sha256(new TextEncoder().encode("admission-test"));
  const key = schnorr.getPublicKey(secret);
  const store = hexToBytes("ab".repeat(32))!;
  const body = concat([
    new TextEncoder().encode("OBAX"),
    Uint8Array.of(1),
    key,
    u64(4),
    u32(20261123),
    u64(9),
    u16(1),
    store,
  ]);
  const signed = bytesToHex(concat([body, schnorr.sign(tagged("ob/admission-index/v1", body), secret)]));
  assert.deepEqual(verifyAdmissionIndex(signed, bytesToHex(key)), ["ab".repeat(32)]);
  assert.equal(verifyAdmissionIndex(`${signed.slice(0, -2)}00`, bytesToHex(key)), null);
  assert.equal(verifyAdmissionIndex(signed, "cd".repeat(32)), null);
});
