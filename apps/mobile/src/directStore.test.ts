import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { DIRECT_GET, NOT_A_STORE, UNSIGNED, UNREACHABLE, catalogFromPublic, directBrowseBase, directImageUrl, fetchPublicCatalog, isPlainText, setDirectBrowse, shownListing, shownStore, storeBase, storeChoice, storeFromLocation } from "./directStore.ts";
import { verifyListingRecord, verifyStoreRecord } from "./records.ts";

const onion = `${"a".repeat(56)}.onion`;
type Raw = { stores: { key: string; name: string; record: string }[]; listings: { storeId: string; record: string; title: string }[] };
const raw = (): Raw => JSON.parse(readFileSync(new URL("./catalog.json", import.meta.url), "utf8")) as Raw;

test("a store link is an onion or an https node, and nothing else", () => {
  assert.equal(storeBase(`http://${onion}`), `http://${onion}`);
  assert.equal(storeBase(`  HTTP://${onion.toUpperCase()}/public/store/  `), `http://${onion}`);
  assert.equal(storeBase(`https://shop.example/public/store`), "https://shop.example");
  assert.equal(storeBase("https://shop.example:8443/"), "https://shop.example:8443");
  for (const rejected of ["", "http://shop.example", "https://127.0.0.1", "https://localhost", `https://${onion}`, `http://${onion}:8080`, `http://${onion}/catalog`, "https://user:pass@shop.example", "https://shop.example?x=1", "https://192.0.2.1", "not a url"]) {
    assert.equal(storeBase(rejected), null, rejected);
  }
});

test("an opened link names the store, and a home-node onion does not", () => {
  const page = { search: `?store=http://${onion}`, hostname: "localhost", protocol: "http:" };
  assert.deepEqual(storeFromLocation(page, false), { url: `http://${onion}`, rejected: false });
  assert.deepEqual(storeFromLocation(page, true), { url: `http://${onion}`, rejected: false });
  assert.deepEqual(storeFromLocation({ search: "", hostname: onion, protocol: "http:", port: "" }, false), { url: null, rejected: false }, "the onion page is the market");
  assert.deepEqual(storeFromLocation({ search: "", hostname: onion, protocol: "http:" }, true), { url: null, rejected: false });
  assert.deepEqual(storeFromLocation({ search: "?store=nope", hostname: onion, protocol: "http:" }, false), { url: null, rejected: true });
  assert.deepEqual(storeChoice({ search: "", hostname: "localhost", protocol: "http:" }, false, "https://shop.example/"), { url: "https://shop.example", rejected: false });
  assert.deepEqual(storeChoice(page, false, "https://other.example"), { url: `http://${onion}`, rejected: false });
});

test("a public store keeps signed listings from that store and drops the rest", () => {
  const { stores, listings } = raw();
  const store = verifyStoreRecord(stores[0].record);
  assert.ok(store);
  const own = listings.filter((item) => item.storeId === store.key);
  const other = listings.filter((item) => item.storeId !== store.key);
  const catalog = catalogFromPublic({ store: stores[0].record, listings: [...own.map((item) => item.record), ...other.map((item) => item.record), "00", 4] });
  assert.equal(catalog.listings.length, own.length);
  assert.equal(catalog.rejected, other.length + 2);
  assert.equal(catalog.store.name, stores[0].name);
  assert.equal(catalog.listings[0].title, own[0].title);
  assert.equal(catalog.listings.every((item) => item.storeId === store.key), true);
  assert.equal(catalog.moderator, null);
  assert.equal(catalog.stores[0].own, false);

  const tampered = own[0].record;
  const flipped = `${tampered.slice(0, 20)}00${tampered.slice(22)}`;
  const dropped = catalogFromPublic({ store: stores[0].record, listings: [flipped] });
  assert.equal(dropped.listings.length, 0);
  assert.equal(dropped.rejected, 1);
  assert.throws(() => catalogFromPublic({ store: "00", listings: [] }), new Error(UNSIGNED));
  assert.throws(() => catalogFromPublic({ store: stores[0].record, listings: Array(501).fill("00") }), /too many listings/);
  assert.throws(() => catalogFromPublic(null), /could not be read/);
});

test("hostile text is not shown, and a plain title is", () => {
  assert.equal(isPlainText("Wool blankets", false), true);
  assert.equal(isPlainText("a line\nand another", true), true);
  assert.equal(isPlainText("a line\nand another", false), false);
  assert.equal(isPlainText("abc\u202Eexe", true), false);
  const { stores, listings } = raw();
  const store = verifyStoreRecord(stores[0].record);
  const record = verifyListingRecord(listings.find((item) => item.storeId === store?.key)?.record);
  assert.ok(store && record);
  assert.equal(shownListing(record, store)?.title, record.title);
  assert.equal(shownListing({ ...record, title: "abc\u202Eexe" }, store), null);
  assert.equal(shownListing({ ...record, description: "see\u2066me" }, store), null);
  assert.equal(shownListing({ ...record, seller: "Someone else" }, store), null);
  const marked = shownListing({ ...record, title: `Z${"\u0336".repeat(5)}` }, store);
  assert.ok(marked);
  assert.equal(marked.title.length < `Z${"\u0336".repeat(5)}`.length, true);
  assert.equal(shownStore({ ...store, name: "a\u2066b" }, 1), null);
  assert.equal(shownStore(store, 2)?.name, store.name);
});

test("the page asks the seller's node with no cookies and does not follow a redirect", async () => {
  const { stores, listings } = raw();
  const store = verifyStoreRecord(stores[0].record);
  assert.ok(store);
  const own = listings.filter((item) => item.storeId === store.key).map((item) => item.record);
  let asked = "";
  let init: RequestInit | undefined;
  const catalog = await fetchPublicCatalog(`https://shop.example/public/store`, async (url, options) => {
    asked = url;
    init = options;
    return new Response(JSON.stringify({ store: stores[0].record, listings: own }), { status: 200 });
  });
  assert.equal(asked, "https://shop.example/public/store");
  assert.equal(init?.credentials, DIRECT_GET.credentials);
  assert.equal(init?.redirect, "error");
  assert.equal(init?.referrerPolicy, "no-referrer");
  assert.equal(catalog.listings.length, own.length);

  await assert.rejects(fetchPublicCatalog("https://shop.example", async () => new Response(JSON.stringify({ error: "This node is not selling." }), { status: 404 })), /no open store/);
  await assert.rejects(fetchPublicCatalog("https://shop.example", async () => new Response(JSON.stringify({ error: "<script>alert(1)</script>" }), { status: 500 })), (error: Error) => error.message === UNREACHABLE);
  await assert.rejects(fetchPublicCatalog("https://down.example", async () => { throw new Error("failed"); }), (error: Error) => error.message === UNREACHABLE);
  await assert.rejects(fetchPublicCatalog("http://shop.example"), (error: Error) => error.message === NOT_A_STORE);
});

test("images are read from the store being browsed", () => {
  const hash = "ab".repeat(32);
  setDirectBrowse(`http://${onion}/public/store`);
  assert.equal(directBrowseBase(), `http://${onion}`);
  assert.equal(directImageUrl(`http://${onion}`, hash), `http://${onion}/public/media/${hash}`);
  assert.equal(directImageUrl(`http://${onion}`, "zz"), null);
  setDirectBrowse(null);
  assert.equal(directBrowseBase(), null);
});
