import assert from "node:assert/strict";
import { test } from "node:test";

import { conditionChoices, filterListings, matchingStores, narrowingActive, setSpecificChoice, specificChoices, storesWithoutListings, type StoreSearch } from "./search.ts";

const cedar: StoreSearch = { key: "aa", name: "Cedar Workshop", region: "Maine", tagline: "Wooden tools", about: "Spoons and bowls" };
const maple: StoreSearch = { key: "bb", name: "Maple Workshop", region: "Vermont", tagline: "", about: "" };

test("a store name matches before its listings are held", () => {
  assert.deepEqual(matchingStores([cedar, maple], "cedar").map((store) => store.name), ["Cedar Workshop"]);
  assert.deepEqual(matchingStores([cedar], "maine").map((store) => store.key), ["aa"]);
  assert.deepEqual(matchingStores([cedar], "bowls").map((store) => store.key), ["aa"]);
  assert.deepEqual(matchingStores([cedar], "cedar spoon"), []);
  assert.deepEqual(
    filterListings([{ title: "Cedar spoon", category: "Home", shipFrom: "Maine", kind: "physical", description: "", seller: "Cedar Workshop" } as never], "cedar spoon").map((listing) => listing.title),
    ["Cedar spoon"],
  );
});

test("a store with no listings is named and is not a good", () => {
  const named = storesWithoutListings([cedar, maple], [{ storeId: "bb" }]);
  assert.deepEqual(named.map((store) => store.key), ["aa"]);
});

test("price, condition and signed item specifics narrow a search", () => {
  const row = (title: string, priceSats: number, commerce: unknown) => ({ title, category: "Home", shipFrom: "Maine", kind: "physical", description: "", seller: "Cedar", priceSats, commerce });
  const mug = row("Mug", 5_000, { condition: "good", specifics: { brand: "Marlow", color: "blue" } });
  const bowl = row("Bowl", 20_000, { condition: "new", specifics: { "country-of-origin": "Japan" } });
  const plain = row("Plain", 1_000, null);
  const titles = (narrowing: Parameters<typeof filterListings>[3]) => filterListings([mug, bowl, plain] as never, "", [], narrowing).map((listing) => listing.title);
  assert.deepEqual(titles({ min: "4000", max: "6000" }), ["Mug"]);
  assert.deepEqual(titles({ min: "", max: "" }), ["Mug", "Bowl", "Plain"]);
  assert.deepEqual(titles({ condition: "new" }), ["Bowl"]);
  assert.deepEqual(titles({ specifics: { Brand: "Marlow" } }), ["Mug"]);
  assert.deepEqual(titles({ specifics: { "Country of origin": "Japan" } }), ["Bowl"]);
  assert.deepEqual(filterListings([mug, bowl] as never, "bowl", [], { condition: "good" }).map((listing) => listing.title), []);
  assert.equal(narrowingActive({ min: "4000", max: "0", condition: "good", specifics: { Brand: "Marlow" } }), 4);
  assert.equal(narrowingActive({ min: "", condition: "", specifics: {} }), 0);
});

test("unknown or hostile text does not become a filter or throw", () => {
  const crash = { toLowerCase() { throw new Error("crash"); } };
  const specifics = Object.create({ constructor: crash, toString: crash });
  Object.defineProperty(specifics, "brand", { value: "\u202eNavy", enumerable: true });
  Object.defineProperty(specifics, "constructor", { value: "boom", enumerable: true });
  Object.defineProperty(specifics, "seller-note", { value: "free sidebar", enumerable: true });
  const listing = { title: "Mug", priceSats: 5_000, commerce: { condition: "good", specifics } };
  const choices = specificChoices([listing, { commerce: { condition: "__proto__", specifics: { "not-a-key": "x".repeat(500) } } }]);
  assert.deepEqual(choices.map((choice) => choice.key), ["Brand"]);
  assert.deepEqual(choices[0].values.map((value) => value.label), ["Navy"]);
  assert.deepEqual(conditionChoices([{ commerce: { condition: "fair" } }, { commerce: { condition: "constructor" } }, { commerce: null }]), ["fair"]);
  const hostile = Object.create(null) as Record<string, string>;
  hostile.constructor = "boom";
  hostile.__proto__ = "x";
  assert.doesNotThrow(() => filterListings([listing, { title: "Plain", priceSats: 1, commerce: null }] as never, "", [], {
    min: "__proto__", max: "constructor", condition: "not-a-condition", specifics: hostile,
  }));
  assert.deepEqual(filterListings([listing] as never, "", [], { specifics: { Brand: "\u202eNavy" } }).map((item) => item.title), ["Mug"]);
  const inherited = Object.create({ constructor: crash, toString: crash });
  Object.defineProperty(inherited, "brand", { value: "Marlow", enumerable: true });
  assert.deepEqual(filterListings([{ title: "Safe", priceSats: 5, commerce: { condition: "good", specifics: inherited } }] as never, "", [], { specifics: hostile }).map((item) => item.title), ["Safe"]);
  const proto = Object.getPrototypeOf({});
  assert.deepEqual(setSpecificChoice({ constructor: "boom" } as never, "__proto__", "admin"), {});
  assert.equal(Object.hasOwn(proto, "admin"), false);
  assert.deepEqual(setSpecificChoice({ Brand: "Marlow" }, "Brand", ""), {});
});
