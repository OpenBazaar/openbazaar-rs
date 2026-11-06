import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import type { ListingHeader } from "../catalog";
import { DEFAULT_SAFETY, matchedKeyword, parseKeywords, parseSafety, safetyVerdict, visibleListings, type SafetySettings } from "./filter.ts";

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

const SAFE: SafetySettings = { safeMode: true, blockedKeywords: [] };
const OPEN: SafetySettings = { safeMode: false, blockedKeywords: [] };

test("safe mode is on for a new user and when storage is unreadable", () => {
  assert.equal(DEFAULT_SAFETY.safeMode, true);
  assert.deepEqual(parseSafety(null), DEFAULT_SAFETY);
  assert.deepEqual(parseSafety("not json"), DEFAULT_SAFETY);
  assert.equal(parseSafety("{}").safeMode, true);
  assert.equal(parseSafety(JSON.stringify({ safeMode: false })).safeMode, false);
  assert.deepEqual(parseSafety(JSON.stringify({ safeMode: true, blockedKeywords: ["Knife", 4, "knife"] })).blockedKeywords, ["knife"]);
});

test("safe mode hides adult and sensitive listings and shows general and unrated ones", () => {
  assert.equal(safetyVerdict(listing({ contentRating: "adult" }), "general", SAFE).hidden, true);
  assert.equal(safetyVerdict(listing({ contentRating: "sensitive" }), "general", SAFE).hidden, true);
  assert.equal(safetyVerdict(listing({ contentRating: "general" }), "general", SAFE).hidden, false);
  assert.equal(safetyVerdict(listing({ contentRating: "unrated" }), "unrated", SAFE).hidden, false);
});

test("with safe mode off, ratings do not hide anything", () => {
  assert.equal(safetyVerdict(listing({ contentRating: "adult" }), "adult", OPEN).hidden, false);
  assert.equal(safetyVerdict(listing({ title: "XXX DVD" }), "general", OPEN).hidden, false);
});

test("an adult store hides its general listings in safe mode", () => {
  const verdict = safetyVerdict(listing(), "adult", SAFE);
  assert.deepEqual(verdict, { hidden: true, reason: "The store rated it adult." });
  assert.equal(safetyVerdict(listing(), "adult", OPEN).hidden, false);
});

test("the buyer's blocklist applies even with safe mode off", () => {
  const settings = { safeMode: false, blockedKeywords: ["mug"] };
  assert.deepEqual(safetyVerdict(listing(), "general", settings), { hidden: true, reason: "It matches your blocked word “mug”." });
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

test("the built-in list catches unlabeled listings in safe mode only", () => {
  const dvd = listing({ title: "XXX DVD", contentRating: "unrated" });
  const verdict = safetyVerdict(dvd, "unrated", SAFE);
  assert.equal(verdict.hidden, true);
  assert.match(verdict.hidden ? verdict.reason : "", /xxx/);
  assert.equal(safetyVerdict(listing({ description: "Smells of cocaine." }), "general", SAFE).hidden, true);
});

test("the built-in list leaves ordinary goods alone", () => {
  for (const title of ["XXXL work shirt", "The Heroine's Journey", "Glue gun", "Adult wool socks", "Kitchen knife", "Wine glass", "Poker chips"]) {
    assert.equal(safetyVerdict(listing({ title }), "general", SAFE).hidden, false, title);
  }
});

test("safe mode hides nothing in the fixture shop", () => {
  const catalog = JSON.parse(readFileSync(new URL("../catalog.json", import.meta.url), "utf8")) as { store: { contentRating: "general" }; listings: ListingHeader[] };
  assert.ok(catalog.listings.length >= 30);
  assert.equal(visibleListings(catalog.listings, catalog.store.contentRating, SAFE).length, catalog.listings.length);
});

test("parseKeywords splits on commas and lines, trims, lowercases, and drops repeats", () => {
  assert.deepEqual(parseKeywords(" Knife, knife\n\nSex  Toy ,, "), ["knife", "sex toy"]);
  assert.deepEqual(parseKeywords(""), []);
});
