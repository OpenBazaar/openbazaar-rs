import assert from "node:assert/strict";
import { test } from "node:test";

import { parseBuyerOrder } from "./catalog.ts";
import { languagesOf, parseModerators, parseOwnModerator, shortKey, trackRecordText } from "./moderators.ts";

const id = "8f".repeat(32);
const known = (extra: Record<string, unknown> = {}) => ({ id, name: "Ines Varga", description: "Rules within two days.", terms: "", languages: ["en", "pt-br"], feeSats: 1500, address: "http://127.0.0.1:9000", builtIn: false, accepted: true, trackRecord: { rulings: 3, stores: 2, checkedAt: 1700000000 }, ...extra });

test("moderators parse as the node sends them", () => {
  const [moderator] = parseModerators({ moderators: [known()], escrow: true });
  assert.equal(moderator.id, id);
  assert.equal(moderator.name, "Ines Varga");
  assert.equal(moderator.feeSats, 1500);
  assert.deepEqual(moderator.languages, ["en", "pt-br"]);
  assert.deepEqual(moderator.trackRecord, { rulings: 3, stores: 2, checkedAt: 1700000000 });
  assert.deepEqual(parseModerators({ moderators: [] }), []);
});

test("a moderator without a key is an error, whatever its name", () => {
  assert.throws(() => parseModerators({}));
  assert.throws(() => parseModerators({ moderators: [known({ id: "Ines Varga" })] }));
  assert.throws(() => parseModerators({ moderators: [{ name: "Ines Varga" }] }));
});

test("the track record says only what was verified", () => {
  const text = (extra: Record<string, unknown>) => trackRecordText(parseModerators({ moderators: [known(extra)] })[0]);
  assert.equal(text({}), "3 signed public rulings across 2 stores.");
  assert.equal(text({ trackRecord: { rulings: 1, stores: 1, checkedAt: 5 } }), "1 signed public ruling across 1 store.");
  assert.equal(text({ trackRecord: { rulings: 0, stores: 0, checkedAt: 5 } }), "No public rulings yet.");
  assert.equal(text({ trackRecord: null }), "Track record not checked yet.");
  assert.equal(text({ builtIn: true, trackRecord: { rulings: 9, stores: 9, checkedAt: 5 } }), "Test moderator with public keys. No track record.");
});

test("the desk's own profile parses before and after it is named", () => {
  const fresh = parseOwnModerator({ id: null, published: false, address: null, builtIn: { id: "f9".repeat(32), name: "June Pell" } });
  assert.deepEqual([fresh.id, fresh.name, fresh.published, fresh.builtIn?.name], [null, "", false, "June Pell"]);
  const named = parseOwnModerator({ id, name: "Ines Varga", description: "", terms: "", languages: ["en"], feeSats: 1500, minFeeSats: 330, published: true, address: "http://127.0.0.1:9000", trackRecord: { rulings: 2, stores: 1 }, builtIn: null });
  assert.deepEqual([named.id, named.feeSats, named.minFeeSats, named.rulings, named.stores, named.builtIn], [id, 1500, 330, 2, 1, null]);
  assert.throws(() => parseOwnModerator({ name: "Ines Varga" }));
});

test("an escrow order names the moderator the buyer chose, and an older order names none", () => {
  const order = (escrow: Record<string, unknown>) => parseBuyerOrder({ id: "order-1", created: 1, seller: "Marlow Goods", storeId: "aa", totalSats: 12_000, settlement: "escrow", status: "Awaiting vendor terms", lines: [{ listingHash: "bb", title: "Cup", quantity: 1, priceSats: 12_000 }], escrow }).escrow;
  assert.deepEqual(order({ stage: "requested", moderatorFeeSats: 1500, moderator: { id, name: "Ines Varga", builtIn: false, profile: "00" } }), { moderator: { id, name: "Ines Varga", builtIn: false }, fundingSats: 0, timeoutHeight: 0, address: "", outpoint: "", signaturesRequired: 0, moderatorFeeSats: 1500 });
  assert.equal(order({ stage: "requested", moderatorFeeSats: 1000 })?.moderator, undefined);
});

test("keys are shortened and language tags are split", () => {
  assert.equal(shortKey(id), "8f8f8f8f…8f8f8f");
  assert.deepEqual(languagesOf("EN, pt-br  en"), ["en", "pt-br"]);
  assert.deepEqual(languagesOf(" "), []);
});
