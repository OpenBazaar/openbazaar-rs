import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import type { ListingHeader } from "../catalog";
import { DEFAULT_SAFETY, matchedKeyword, parseKeywords, parseSafety, safetyVerdict, toggleHidden, visibleListings, type SafetySettings } from "./filter.ts";

function listing(overrides: Partial<ListingHeader> = {}): ListingHeader {
  return {
    hash: "h",
    storeId: "s",
    title: "Stoneware mug",
    category: "Home",
    priceSats: 11_000,
    shipFrom: "Oregon",
    kind: "physical",
    acceptsModerator: true,
    description: "A 12 ounce stoneware mug with a dull white glaze.",
    seller: "Marlow Goods",
    stock: 7,
    listedOn: 20260901,
    product: "",
    shippingSats: 800,
    freeOverSats: 30_000,
    contentRating: "general",
    contentTags: [],
    ...overrides,
  };
}

const SAFE: SafetySettings = DEFAULT_SAFETY;
const OPEN: SafetySettings = { ...DEFAULT_SAFETY, safeMode: false };
const action = (item: ListingHeader, settings: SafetySettings, store: ListingHeader["contentRating"] = "general") => safetyVerdict(item, store, settings).action;

test("safe mode is on for a new user and when storage is unreadable", () => {
  assert.equal(DEFAULT_SAFETY.safeMode, true);
  assert.deepEqual(parseSafety(null), DEFAULT_SAFETY);
  assert.deepEqual(parseSafety("not json"), DEFAULT_SAFETY);
  assert.deepEqual(parseSafety("null"), DEFAULT_SAFETY);
  assert.equal(parseSafety("{}").safeMode, true);
  assert.equal(parseSafety(JSON.stringify({ safeMode: false })).safeMode, false);
});

test("first-version settings migrate with the new switches at their defaults", () => {
  const migrated = parseSafety(JSON.stringify({ safeMode: false, blockedKeywords: ["Knife", 4, "knife"] }));
  assert.deepEqual(migrated, { ...DEFAULT_SAFETY, safeMode: false, blockedKeywords: ["knife"] });
});

test("unknown switch values fall back to defaults", () => {
  const parsed = parseSafety(JSON.stringify({ sensitive: "blur", adult: "cover", tags: { drugs: "hide", weapons: "maybe", lasers: "hide" } }));
  assert.equal(parsed.sensitive, "hide");
  assert.equal(parsed.adult, "hide");
  assert.equal(parsed.tags.drugs, "hide");
  assert.equal(parsed.tags.weapons, "show");
  assert.equal("lasers" in parsed.tags, false);
});

test("default safe mode hides adult and sensitive listings and shows general and unrated ones", () => {
  assert.equal(action(listing({ contentRating: "adult" }), SAFE), "hide");
  assert.equal(action(listing({ contentRating: "sensitive" }), SAFE), "hide");
  assert.equal(action(listing({ contentRating: "general" }), SAFE), "show");
  assert.equal(action(listing({ contentRating: "unrated" }), SAFE, "unrated"), "show");
});

test("sensitive can be shown, covered, or hidden", () => {
  const sensitive = listing({ contentRating: "sensitive" });
  assert.equal(action(sensitive, { ...SAFE, sensitive: "show" }), "show");
  assert.deepEqual(safetyVerdict(sensitive, "general", { ...SAFE, sensitive: "cover" }), { action: "cover", reason: "The seller rated it sensitive." });
  assert.equal(action(sensitive, { ...SAFE, sensitive: "hide" }), "hide");
});

test("adult can be shown or hidden", () => {
  assert.equal(action(listing({ contentRating: "adult" }), { ...SAFE, adult: "show" }), "show");
  assert.equal(action(listing({ contentRating: "adult" }), { ...SAFE, adult: "hide" }), "hide");
});

test("each tag hides on its own and beats a softer rating", () => {
  const knife = listing({ contentRating: "general", contentTags: ["weapons"] });
  assert.equal(action(knife, SAFE), "show");
  assert.deepEqual(safetyVerdict(knife, "general", { ...SAFE, tags: { ...SAFE.tags, weapons: "hide" } }), { action: "hide", reason: "It is tagged weapons, which you hide." });
  assert.equal(action(knife, { ...SAFE, tags: { ...SAFE.tags, drugs: "hide" } }), "show");
  const covered = listing({ contentRating: "sensitive", contentTags: ["weapons"] });
  assert.equal(action(covered, { ...SAFE, sensitive: "cover", tags: { ...SAFE.tags, weapons: "hide" } }), "hide");
});

test("with safe mode off, ratings and tags do not hide anything", () => {
  const all: SafetySettings = { ...OPEN, sensitive: "hide", adult: "hide", tags: { ...SAFE.tags, nudity: "hide" } };
  assert.equal(action(listing({ contentRating: "adult", contentTags: ["nudity"] }), all, "adult"), "show");
  assert.equal(action(listing({ title: "XXX DVD" }), all), "show");
});

test("an adult store hides its general listings in safe mode", () => {
  assert.deepEqual(safetyVerdict(listing(), "adult", SAFE), { action: "hide", reason: "The store rated it adult." });
  assert.equal(action(listing(), OPEN, "adult"), "show");
});

test("the buyer's blocklist applies even with safe mode off", () => {
  const settings = { ...OPEN, blockedKeywords: ["mug"] };
  assert.deepEqual(safetyVerdict(listing(), "general", settings), { action: "hide", reason: "It matches your blocked word “mug”." });
});

test("blocked words match title, category, description, and tags, ignoring case", () => {
  assert.equal(matchedKeyword(listing({ title: "Big MUG" }), ["mug"]), "mug");
  assert.equal(matchedKeyword(listing(), ["home"]), "home");
  assert.equal(matchedKeyword(listing(), ["glaze"]), "glaze");
  assert.equal(matchedKeyword(listing({ contentTags: ["weapons"] }), ["weapons"]), "weapons");
  assert.equal(matchedKeyword(listing({ description: undefined }), ["glaze"]), null);
});

test("blocked words match whole words only", () => {
  assert.equal(matchedKeyword(listing({ title: "Party cart" }), ["art"]), null);
  assert.equal(matchedKeyword(listing({ title: "Mugs", description: "Two of them." }), ["mug"]), null);
  assert.equal(matchedKeyword(listing({ title: "Café crème" }), ["CAFÉ"]), "CAFÉ");
});

test("a blocked phrase matches across punctuation but not split apart", () => {
  assert.equal(matchedKeyword(listing({ title: "Sex-toy cleaner" }), ["sex toy"]), "sex toy");
  assert.equal(matchedKeyword(listing({ title: "Toy for sex ed" }), ["sex toy"]), null);
});

test("the built-in list follows the sensitive switch in safe mode", () => {
  const dvd = listing({ title: "XXX DVD", contentRating: "unrated" });
  const verdict = safetyVerdict(dvd, "unrated", SAFE);
  assert.equal(verdict.action, "hide");
  assert.match(verdict.action === "show" ? "" : verdict.reason, /xxx/);
  assert.equal(action(dvd, { ...SAFE, sensitive: "cover" }, "unrated"), "cover");
  assert.equal(action(dvd, { ...SAFE, sensitive: "show" }, "unrated"), "show");
  assert.equal(action(listing({ description: "Smells of cocaine." }), SAFE), "hide");
});

test("the built-in list leaves ordinary goods alone", () => {
  for (const title of ["XXXL work shirt", "The Heroine's Journey", "Glue gun", "Adult wool socks", "Kitchen knife", "Wine glass", "Poker chips"]) {
    assert.equal(action(listing({ title }), SAFE), "show", title);
  }
});

test("safe mode hides nothing in the fixture shop", () => {
  const catalog = JSON.parse(readFileSync(new URL("../catalog.json", import.meta.url), "utf8")) as { store: { contentRating: "general" }; listings: ListingHeader[] };
  assert.ok(catalog.listings.length >= 30);
  assert.equal(visibleListings(catalog.listings, catalog.store.contentRating, SAFE).length, catalog.listings.length);
});

const STORE = "a".repeat(64);
const HASH = "b".repeat(64);

test("a hidden listing or seller is hidden even with safe mode off", () => {
  const mug = listing({ hash: HASH, storeId: STORE });
  assert.deepEqual(safetyVerdict(mug, "general", { ...OPEN, hiddenListings: [{ id: HASH, label: "Stoneware mug" }] }), { action: "hide", reason: "You hid this listing." });
  assert.deepEqual(safetyVerdict(mug, "general", { ...OPEN, hiddenStores: [{ id: STORE, label: "Marlow Goods" }] }), { action: "hide", reason: "You hid this seller." });
  assert.equal(action(listing({ hash: "c".repeat(64), storeId: STORE }), { ...OPEN, hiddenListings: [{ id: HASH, label: "x" }] }), "show");
});

test("a seller is hidden by signing key, not by name", () => {
  const settings = { ...OPEN, hiddenStores: [{ id: STORE, label: "Marlow Goods" }] };
  assert.equal(action(listing({ storeId: "d".repeat(64), seller: "Marlow Goods" }), settings), "show");
  assert.equal(action(listing({ storeId: STORE, seller: "Someone Else" }), settings), "hide");
});

test("toggleHidden adds and removes by id, so unhiding restores the listing", () => {
  const hidden = toggleHidden([], { id: HASH, label: "Mug" });
  assert.equal(action(listing({ hash: HASH }), { ...OPEN, hiddenListings: hidden }), "hide");
  const restored = toggleHidden(hidden, { id: HASH, label: "renamed" });
  assert.deepEqual(restored, []);
  assert.equal(action(listing({ hash: HASH }), { ...OPEN, hiddenListings: restored }), "show");
});

test("stored hidden lists keep only well-formed, unique keys", () => {
  const parsed = parseSafety(JSON.stringify({ hiddenStores: [{ id: STORE, label: "Marlow" }, { id: STORE, label: "dup" }, { id: "Marlow Goods" }, 7], hiddenListings: [{ id: HASH }] }));
  assert.deepEqual(parsed.hiddenStores, [{ id: STORE, label: "Marlow" }]);
  assert.deepEqual(parsed.hiddenListings, [{ id: HASH, label: HASH.slice(0, 12) }]);
});

test("parseKeywords splits on commas and lines, trims, lowercases, and drops repeats", () => {
  assert.deepEqual(parseKeywords(" Knife, knife\n\nSex  Toy ,, "), ["knife", "sex toy"]);
  assert.deepEqual(parseKeywords(""), []);
});
