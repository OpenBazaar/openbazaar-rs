import assert from "node:assert/strict";
import { test } from "node:test";

import {
  formatXmr,
  parsePiconero,
  parseXmrBackend,
  parseXmrPayment,
  parseXmrRate,
  parseXmrStores,
  parseXmrTerms,
  parseXmrWallet,
  railFor,
} from "./xmr.ts";

test("piconero stays exact past the float range", () => {
  assert.equal(parsePiconero("18446744073709551615"), 18446744073709551615n);
  assert.equal(formatXmr(1_500_000_000_000n), "1.5");
  assert.equal(formatXmr(1n), "0.000000000001");
  assert.equal(formatXmr(0n), "0");
  assert.throws(() => parsePiconero(12));
  assert.throws(() => parsePiconero("1.5"));
  assert.throws(() => parsePiconero("-1"));
});

test("a Monero order payment parses, and a bitcoin one is null", () => {
  const payment = {
    rail: "xmr",
    network: "stagenet",
    subaddress: "7Bexample",
    subaddressIndex: 3,
    amountPiconero: "40000000000",
    amountXmr: "0.04",
    uri: "monero:7Bexample?tx_amount=0.04",
    satsPerXmr: 300000,
    priceSats: 12000,
    receivedPiconero: "10000000000",
    confirmationsNeeded: 10,
  };
  const parsed = parseXmrPayment(payment)!;
  assert.equal(parsed.amount, 40_000_000_000n);
  assert.equal(parsed.remaining, 30_000_000_000n, "worked out when the node has not set it yet");
  assert.equal(parsed.overpaid, 0n);
  assert.equal(parseXmrPayment({ ...payment, remainingPiconero: "0", overpaidPiconero: "7" })!.overpaid, 7n);
  assert.equal(parseXmrPayment({ address: "tb1qexample", amountSats: 12000 }), null);
  assert.throws(() => parseXmrPayment({ ...payment, network: "testnet4" }));
});

test("terms, wallet, backend and rate keep the node's values", () => {
  const terms = parseXmrTerms({
    orderId: "o1",
    network: "stagenet",
    amountPiconero: "40000000000",
    amountXmr: "0.04",
    feeCapPiconero: "90000000",
    feeCapXmr: "0.00009",
    destination: "7Bexample",
    expiresInSecs: 120,
  });
  assert.deepEqual(terms, { orderId: "o1", network: "stagenet", amount: 40_000_000_000n, feeCap: 90_000_000n, destination: "7Bexample", expiresInSecs: 120 });

  const locked = parseXmrWallet({ network: "mainnet", enabled: true, keys: "unlock-to-create", balance: null });
  assert.equal(locked.balance, null);
  const wallet = parseXmrWallet({
    network: "stagenet",
    enabled: true,
    keys: "ready",
    primaryAddress: "5example",
    balance: { unlockedPiconero: "5000000000000", pendingPiconero: "0" },
    scannedHeight: 10,
    tip: 10,
    lastSync: 1700000000,
    syncError: null,
  });
  assert.equal(wallet.balance!.unlocked, 5n * 1_000_000_000_000n);

  const off = parseXmrBackend({ backend: "none", network: "stagenet", privacy: "none", reveals: "Monero is off.", nodes: [], proxied: false, tip: null, error: null });
  assert.equal(off.backend, "none");
  assert.throws(() => parseXmrBackend({ ...off, backend: "light-wallet" }), "a light-wallet backend does not exist");
  assert.equal(parseXmrRate(null), null);
  assert.deepEqual(parseXmrRate({ base: "XMR", satsPerXmr: 300000, decimals: 12, currencies: { USD: 180.5, EUR: "x" } }), { satsPerXmr: 300000, currencies: { USD: 180.5 } });
});

test("the stores list says which stores take Monero, and which is the node's own", () => {
  const stores = parseXmrStores({
    network: "stagenet",
    enabled: true,
    stores: [
      { seller: "Brook and Pine", own: true, acceptsXmr: false, problem: "Monero is off on this node. Start it with --xmr-daemon." },
      { seller: "Marlow Goods", acceptsXmr: true },
      { seller: "Hale Paper", acceptsXmr: false },
    ],
  });
  assert.equal(stores.enabled, true);
  assert.deepEqual(stores.stores.map((store) => [store.seller, store.own, store.acceptsXmr]), [["Brook and Pine", true, false], ["Marlow Goods", false, true], ["Hale Paper", false, false]]);
  assert.deepEqual(stores.stores.map((store) => store.problem), ["Monero is off on this node. Start it with --xmr-daemon.", null, null], "why the own store would decline a Monero order now");
  assert.throws(() => parseXmrStores({ network: "testnet4", stores: [] }), "testnet4 is not a Monero network");
});

test("Monero is only a rail for direct pay to a store that accepts it", () => {
  assert.equal(railFor("direct", "xmr", true), "xmr");
  assert.equal(railFor("escrow", "btc", false), "btc");
  assert.throws(() => railFor("escrow", "xmr", true), /Bitcoin-only/);
  assert.throws(() => railFor("direct", "xmr", false), /does not accept/);
});
