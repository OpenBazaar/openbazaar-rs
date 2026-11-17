import assert from "node:assert/strict";
import { test } from "node:test";

import { NodeApiError } from "./catalog.ts";
import { bitcoinAddressOf, bitcoinAddressProblem, heldBack, parseBitcoinQuote, parseSats, quoteExpired, sendError } from "./walletSend.ts";

const TESTNET = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
const MAINNET = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";

test("amounts are whole positive sats, with separators allowed", () => {
  assert.equal(parseSats("25000"), 25000);
  assert.equal(parseSats(" 25,000 "), 25000);
  assert.equal(parseSats("1_000"), 1000);
  for (const bad of ["", "0", "-5", "1.5", "1e5", "abc", "99999999999999999999"]) assert.equal(parseSats(bad), null, bad);
});

test("an address on the other network is refused before the node is asked", () => {
  assert.equal(bitcoinAddressProblem(TESTNET, "testnet4"), null);
  assert.equal(bitcoinAddressProblem(MAINNET, "mainnet"), null);
  assert.match(bitcoinAddressProblem(MAINNET, "testnet4") ?? "", /mainnet address.*testnet4/);
  assert.match(bitcoinAddressProblem(TESTNET, "mainnet") ?? "", /testnet4 address.*real bitcoin/);
  assert.match(bitcoinAddressProblem("1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2", "testnet4") ?? "", /mainnet/);
  assert.match(bitcoinAddressProblem("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080", "testnet4") ?? "", /regtest/);
  assert.equal(bitcoinAddressProblem("", "testnet4"), "Enter the address to send to.");
  assert.equal(bitcoinAddressProblem("hello", "testnet4"), "That is not a bitcoin address.");
});

test("a bitcoin: URI gives its address", () => {
  assert.equal(bitcoinAddressOf(`bitcoin:${TESTNET}?amount=0.001`), TESTNET);
  assert.equal(bitcoinAddressOf(`  ${TESTNET} `), TESTNET);
  assert.equal(bitcoinAddressProblem(`bitcoin:${TESTNET}`, "testnet4"), null);
});

test("a quote keeps the node's terms and refuses an incomplete one", () => {
  const quote = {
    quote: "q1", network: "testnet4", address: TESTNET, amountSats: 25000, feeSats: 282, feeCapSats: 423,
    feeRateSatVb: 2, totalSats: 25282, max: false, linkCoins: false, heldBackSats: 0, expiresInSecs: 60,
  };
  assert.deepEqual(parseBitcoinQuote(quote), quote);
  assert.throws(() => parseBitcoinQuote({ ...quote, quote: "" }));
  assert.throws(() => parseBitcoinQuote({ ...quote, network: "signet" }));
  assert.throws(() => parseBitcoinQuote({ ...quote, feeSats: -1 }));
});

test("send errors name the cause", () => {
  assert.equal(quoteExpired(new NodeApiError(409, "That quote is used or expired.")), true);
  assert.equal(quoteExpired(new NodeApiError(400, "x")), false);
  assert.equal(heldBack("Not enough funds. 500 sats of change from earlier payments is held back so..."), true);
  assert.match(sendError(new NodeApiError(502, "mock relay is down"), "x"), /could not reach the network.*mock relay is down/);
  assert.match(sendError(new NodeApiError(400, "That passphrase is wrong.", "wrong_passphrase", 4), "x"), /Wait 4 seconds/);
  assert.equal(sendError("nope", "fallback"), "fallback");
});
