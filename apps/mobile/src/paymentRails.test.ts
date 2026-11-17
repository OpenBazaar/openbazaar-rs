import assert from "node:assert/strict";
import { test } from "node:test";

import { LIGHTNING_OFF, MONERO_OFF, STORE_NOT_OPEN, ZCASH_OFF, lightningOffer, moneroOffer, zcashOffer, type LightningRail } from "./paymentRails.ts";

const ready = { enabled: true, accepts: false, problem: null };
const lightning = (over: Partial<LightningRail> = {}): LightningRail => ({
  ...ready,
  blocked: null,
  lspNotice: "The LSP sees the amount and time of each payment.",
  lspAcknowledged: false,
  ...over,
});

test("an open testnet4 store can turn on Monero, Lightning, and Zcash", () => {
  const monero = moneroOffer(true, ready);
  const zcash = zcashOffer(true, ready);
  const ln = lightningOffer(true, lightning());
  assert.equal(monero.choosable, true);
  assert.equal(monero.on, false);
  assert.equal(monero.reason, null);
  assert.equal(zcash.choosable, true);
  assert.equal(zcash.on, false);
  assert.equal(ln.choosable, true);
  assert.equal(ln.needsLspAck, true);
});

test("a rail that is already on stays on, and a second Lightning opt-in skips the notice", () => {
  assert.equal(moneroOffer(true, { ...ready, accepts: true }).on, true);
  assert.equal(zcashOffer(true, { ...ready, accepts: true }).on, true);
  const again = lightningOffer(true, lightning({ accepts: true, lspAcknowledged: true }));
  assert.equal(again.on, true);
  assert.equal(again.needsLspAck, false);
});

test("a missing daemon, indexer, or Lightning flag disables the switch and gives the node's reason", () => {
  assert.deepEqual(moneroOffer(true, { enabled: false, accepts: false, problem: null }), {
    on: false, choosable: false, reason: MONERO_OFF, problem: null, needsLspAck: false,
  });
  assert.equal(zcashOffer(true, { enabled: false, accepts: false, problem: null }).reason, ZCASH_OFF);
  assert.equal(lightningOffer(true, lightning({ enabled: false, lspNotice: null })).reason, LIGHTNING_OFF);
  const mainnet = "Lightning on mainnet stays off until peers, the chain source, and gossip go through a proxy.";
  assert.equal(lightningOffer(true, lightning({ enabled: false, blocked: mainnet })).reason, mainnet);
});

test("a node's own refusal is shown instead of a generic sentence", () => {
  const synced = "This node has not read the Zcash chain yet. Unlock it if it is locked, then try again in a few minutes.";
  assert.equal(zcashOffer(true, { enabled: false, accepts: false, problem: synced }).reason, synced);
});

test("a store that already takes a rail can turn it off after the node loses that rail", () => {
  const offer = zcashOffer(true, { enabled: false, accepts: true, problem: ZCASH_OFF });
  assert.equal(offer.choosable, true);
  assert.equal(offer.on, true);
  assert.equal(offer.problem, ZCASH_OFF);
});

test("a problem on a live rail is shown and the switch stays usable", () => {
  const price = "This node has no ZEC price right now, so it cannot price a Zcash order. Try again in a few minutes.";
  const offer = zcashOffer(true, { enabled: true, accepts: true, problem: price });
  assert.equal(offer.choosable, true);
  assert.equal(offer.problem, price);
  assert.equal(offer.reason, null);
});

test("the choice waits until the store record exists and until the node has answered", () => {
  assert.equal(moneroOffer(false, ready).reason, STORE_NOT_OPEN);
  assert.equal(moneroOffer(false, ready).choosable, false);
  assert.match(zcashOffer(true, null).reason ?? "", /Checking whether this node can take Zcash/);
  assert.equal(lightningOffer(true, null).choosable, false);
});
