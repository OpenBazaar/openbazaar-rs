import assert from "node:assert/strict";
import { test } from "node:test";

import { lnAddressProblem, lnInvoiceOf, lnInvoiceProblem, parseLnInvoiceQuote, parseLnWithdrawQuote } from "./lnSend.ts";

const SIGNET_INVOICE = "lntbs200u1pjexample";

test("an invoice for another network is refused before the node is asked", () => {
  assert.equal(lnInvoiceProblem(SIGNET_INVOICE, "signet"), null);
  assert.equal(lnInvoiceProblem(`lightning:${SIGNET_INVOICE.toUpperCase()}`, "signet"), null);
  assert.match(lnInvoiceProblem("lnbc200u1pjexample", "signet") ?? "", /for mainnet.*Mutinynet/);
  assert.match(lnInvoiceProblem("lntb200u1pjexample", "signet") ?? "", /for testnet4/);
  assert.match(lnInvoiceProblem("lnbcrt200u1pjexample", "signet") ?? "", /for regtest/);
  assert.equal(lnInvoiceProblem("", "signet"), "Paste the Lightning invoice to pay.");
  assert.match(lnInvoiceProblem("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", "signet") ?? "", /not a Lightning invoice/);
  assert.equal(lnInvoiceOf(` lightning:${SIGNET_INVOICE} `), SIGNET_INVOICE);
});

test("a withdrawal goes to an address on the Lightning network, not the wallet's", () => {
  assert.equal(lnAddressProblem("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", "signet"), null);
  assert.match(lnAddressProblem("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", "signet") ?? "", /not on Mutinynet/);
  assert.equal(lnAddressProblem("", "signet"), "Enter the address to send to.");
  assert.equal(lnAddressProblem("hello", "signet"), "That is not a bitcoin address.");
});

test("quotes keep the node's terms and refuse incomplete ones", () => {
  const invoice = { quote: "q", network: "signet", paymentHash: "ab", amountSats: 20000, feeCapSats: 200, payee: "03ff", description: "coffee", expiresInSecs: 60 };
  assert.deepEqual(parseLnInvoiceQuote(invoice), invoice);
  assert.throws(() => parseLnInvoiceQuote({ ...invoice, network: "liquid" }));
  const withdraw = { quote: "w", network: "signet", address: "tb1q", amountSats: 75000, max: true, expiresInSecs: 60 };
  assert.deepEqual(parseLnWithdrawQuote(withdraw), withdraw);
  assert.throws(() => parseLnWithdrawQuote({ ...withdraw, amountSats: -1 }));
});
