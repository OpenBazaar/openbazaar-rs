import assert from "node:assert/strict";
import { test } from "node:test";

import { parseXmrAmount, parseXmrSendQuote, xmrAddressProblem } from "./xmrSend.ts";

const STAGENET = `5${"a".repeat(94)}`;
const STAGENET_SUB = `7${"b".repeat(94)}`;
const MAINNET = `4${"c".repeat(94)}`;

test("XMR amounts become exact piconero", () => {
  assert.equal(parseXmrAmount("1"), 1_000_000_000_000n);
  assert.equal(parseXmrAmount("0.5"), 500_000_000_000n);
  assert.equal(parseXmrAmount(".000000000001"), 1n);
  assert.equal(parseXmrAmount("1,234.5"), 1_234_500_000_000_000n);
  for (const bad of ["", ".", "0", "0.0", "-1", "1e3", "abc", "0.0000000000001", "18446745"]) assert.equal(parseXmrAmount(bad), null, bad);
});

test("an address on the other network, or an integrated one, is refused before the node is asked", () => {
  assert.equal(xmrAddressProblem(STAGENET, "stagenet"), null);
  assert.equal(xmrAddressProblem(STAGENET_SUB, "stagenet"), null);
  assert.equal(xmrAddressProblem(`monero:${STAGENET}?tx_amount=1`, "stagenet"), null);
  assert.equal(xmrAddressProblem(MAINNET, "mainnet"), null);
  assert.match(xmrAddressProblem(MAINNET, "stagenet") ?? "", /mainnet address.*stagenet coins/);
  assert.match(xmrAddressProblem(STAGENET, "mainnet") ?? "", /stagenet address.*real Monero/);
  assert.match(xmrAddressProblem(`5${"a".repeat(105)}`, "stagenet") ?? "", /Integrated/);
  assert.equal(xmrAddressProblem("", "stagenet"), "Enter the address to send to.");
  assert.equal(xmrAddressProblem("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", "stagenet"), "That is not a Monero address.");
  assert.equal(xmrAddressProblem(`5${"0".repeat(94)}`, "stagenet"), "That is not a Monero address.");
});

test("a quote keeps the node's piconero exactly", () => {
  const quote = parseXmrSendQuote({
    quote: "q1", network: "stagenet", address: STAGENET, amountPiconero: "1000000000000", feePiconero: "30000000",
    feeCapPiconero: "45000000", totalPiconero: "1000030000000", expiresInSecs: 60,
  });
  assert.equal(quote.amount, 1_000_000_000_000n);
  assert.equal(quote.total, 1_000_030_000_000n);
  assert.throws(() => parseXmrSendQuote({ quote: "q1", network: "testnet4" }));
  assert.throws(() => parseXmrSendQuote({ quote: "q1", network: "stagenet", address: STAGENET, amountPiconero: 1 }));
});
