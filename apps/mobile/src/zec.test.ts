import assert from "node:assert/strict";
import { test } from "node:test";

import { formatZec, parseZatoshi, parseZecBackend, parseZecRate, parseZecStores, ZEC_CONFIRMATIONS } from "./zec.ts";

test("zatoshi stay exact and format without float math", () => {
  assert.equal(parseZatoshi("18446744073709551615"), 18446744073709551615n);
  assert.equal(formatZec(150_000_000n), "1.5");
  assert.equal(formatZec(1n), "0.00000001");
  assert.equal(formatZec(1_463_415n), "0.01463415");
  assert.equal(formatZec(0n), "0");
  for (const bad of [12, "1.5", "-1", "", "123456789012345678901"]) assert.throws(() => parseZatoshi(bad), String(bad));
  assert.equal(ZEC_CONFIRMATIONS, 30);
});

test("the stores list says which demo stores take Zcash", () => {
  const stores = parseZecStores({
    network: "testnet",
    enabled: true,
    stores: [
      { seller: "Marlow Goods", acceptsZec: true },
      { seller: "Hale Paper", acceptsZec: false },
    ],
  });
  assert.equal(stores.enabled, true);
  assert.deepEqual(stores.stores.map((store) => [store.seller, store.acceptsZec]), [["Marlow Goods", true], ["Hale Paper", false]]);
  assert.throws(() => parseZecStores({ network: "testnet4", stores: [] }), "testnet4 is not a Zcash network");
});

test("the backend reads off, an own indexer, and a remote one over Tor", () => {
  const off = parseZecBackend({
    enabled: false,
    backend: "none",
    network: "mainnet",
    privacy: "none",
    reveals: "Zcash is off.",
    servers: [],
    pool: "ironwood",
  });
  assert.equal(off.enabled, false);
  assert.equal(off.scannedHeight, null);
  const remote = parseZecBackend({
    enabled: true,
    backend: "remote-indexer",
    network: "testnet",
    privacy: "remote-tor",
    reveals: "The indexer sees a Tor exit.",
    servers: ["https://testnet.zec.rocks:443"],
    active: "https://testnet.zec.rocks:443",
    software: "ECC LightWalletD v0.6.1, lightwallet-protocol v0.5.0",
    proxied: true,
    tip: 4_603_552,
    scannedHeight: 4_603_550,
    error: null,
  });
  assert.equal(remote.proxied, true);
  assert.equal(remote.software, "ECC LightWalletD v0.6.1, lightwallet-protocol v0.5.0");
  assert.equal(remote.tip! - remote.scannedHeight!, 2);
  assert.throws(() => parseZecBackend({ ...remote, backend: "light-wallet-server" }));
});

test("a missing rate means Zcash cannot be priced", () => {
  assert.equal(parseZecRate(null), null);
  const rate = parseZecRate({ base: "ZEC", satsPerZec: 1_229_360, decimals: 8, currencies: { USD: 1229.36, EUR: -1 } });
  assert.deepEqual(rate, { satsPerZec: 1_229_360, currencies: { USD: 1229.36 } });
});
