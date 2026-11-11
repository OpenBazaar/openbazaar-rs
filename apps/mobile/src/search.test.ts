import assert from "node:assert/strict";
import { test } from "node:test";

import { filterListings, matchingStores, storesWithoutListings, type StoreSearch } from "./search.ts";

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
