import assert from "node:assert/strict";
import { test } from "node:test";

import { NodeApiError } from "./catalog.ts";
import { isLocked, parseLockStatus, parseTerms, passphraseError } from "./walletLock.ts";

const status = {
  state: "unlocked",
  autoLockMinutes: 15,
  autoLockRange: [1, 60],
  lockInSecs: 840,
  failedAttempts: 0,
  retryAfterSecs: 0,
  rememberDevice: false,
  rememberAvailable: true,
  backupConfirmed: true,
  minPassphraseChars: 8,
};

test("lock status parses the node's answer and refuses unknown states", () => {
  assert.equal(parseLockStatus(status).state, "unlocked");
  assert.deepEqual(parseLockStatus(status).autoLockRange, [1, 60]);
  assert.equal(parseLockStatus({ ...status, state: "locked", lockInSecs: null }).lockInSecs, null);
  assert.throws(() => parseLockStatus({ ...status, state: "open" }));
  assert.throws(() => parseLockStatus({ ...status, autoLockMinutes: -1 }));
});

test("payment terms keep the node's values and refuse incomplete quotes", () => {
  const terms = {
    orderId: "o1",
    network: "testnet4",
    amountSats: 12_000,
    feeCapSats: 300,
    address: "tb1qexample",
    recipientScript: "0014",
    feeRateSatVb: 2,
    expiresInSecs: 60,
  };
  assert.deepEqual(parseTerms(terms), {
    orderId: "o1",
    network: "testnet4",
    amountSats: 12_000,
    feeCapSats: 300,
    address: "tb1qexample",
    feeRateSatVb: 2,
    expiresInSecs: 60,
  });
  assert.throws(() => parseTerms({ ...terms, address: "" }));
  assert.throws(() => parseTerms({ ...terms, network: "regtest" }));
  assert.throws(() => parseTerms({ ...terms, amountSats: 1.5 }));
});

test("lock errors are told apart by status and code", () => {
  assert.equal(isLocked(new NodeApiError(423, "locked", "locked")), true);
  assert.equal(isLocked(new NodeApiError(400, "wrong", "wrong_passphrase", 0)), false);
  assert.equal(
    passphraseError(new NodeApiError(400, "That passphrase is wrong.", "wrong_passphrase", 4), "x"),
    "That passphrase is wrong. Wait 4 seconds before the next try.",
  );
  assert.equal(passphraseError("not an error", "fallback"), "fallback");
});
