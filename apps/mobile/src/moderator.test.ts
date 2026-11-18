import assert from "node:assert/strict";
import { test } from "node:test";

import { canDecide, canRecuse, caseFeeText, disputeState, parseDisputes, parseDisputeTracking, parseRefundQuote, partyName } from "./moderator.ts";

const letter = (favors: string, ruled = false) => ({ id: "ab".repeat(16), favors, memo: "No parcel arrived.", outpoint: `${"cd".repeat(32)}:0`, receivedHeight: 160400, ruled });

test("disputes parse as the node sends them", () => {
  const [dispute] = parseDisputes({ disputes: [letter("buyer")] });
  assert.deepEqual(dispute, { id: "ab".repeat(16), favors: "buyer", memo: "No parcel arrived.", outpoint: `${"cd".repeat(32)}:0`, receivedHeight: 160400, ruled: false, denied: false, builtIn: false, tracking: null, fee: null });
  assert.equal(parseDisputes({ disputes: [{ ...letter("buyer"), builtIn: true }] })[0].builtIn, true);
  assert.deepEqual(parseDisputes({ disputes: [] }), []);
});

test("a filing fee gates the decision until it is paid, and a denial is its own state", () => {
  const fee = (state: string, extra: Record<string, unknown> = {}) => parseDisputes({ disputes: [{ ...letter("buyer"), fee: { state, amountSats: 2500, dueHeight: 160600, overdue: false, ...extra } }] })[0];
  assert.equal(canDecide(fee("requested")), false);
  assert.equal(canDecide(fee("expired")), false);
  assert.equal(canDecide(fee("paid")), true);
  assert.equal(canDecide(fee("free")), true);
  assert.equal(canDecide(parseDisputes({ disputes: [letter("buyer")] })[0]), true, "an escrow from before filing fees");
  assert.equal(fee("paid", { paidBy: "buyer", rail: "ln" }).fee?.paidBy, "buyer");
  assert.equal(fee("refunded").fee, null);
  assert.equal(caseFeeText(fee("paid", { rail: "onchain" }).fee), "Paid 2,500 sats on-chain · decision due by block 160,600");
  assert.match(caseFeeText(fee("paid", { overdue: true }).fee), /overdue/);
  const denied = { ...fee("paid"), denied: true };
  assert.equal(disputeState(denied, [denied]), "denied");
});

test("a paid case can be recused until a letter of it is decided, and a recusal closes it", () => {
  const fee = (state: string, extra: Record<string, unknown> = {}) => parseDisputes({ disputes: [{ ...letter("buyer"), fee: { state, amountSats: 2500, dueHeight: 160600, overdue: false, ...extra } }] })[0];
  const paid = fee("paid", { paidBy: "buyer" });
  assert.equal(canRecuse(paid, [paid]), true);
  assert.equal(canRecuse(fee("requested"), [fee("requested")]), false, "nothing was paid");
  const ruled = { ...paid, ruled: true };
  assert.equal(canRecuse(paid, [paid, ruled]), false, "a ruling came first");
  const recused = fee("paid", { recused: true, refunds: [{ filer: "buyer", amountSats: 2500, txid: "ef".repeat(32) }, { filer: "nobody" }] });
  assert.deepEqual(recused.fee?.refunds, [{ filer: "buyer", amountSats: 2500, txid: "ef".repeat(32), owed: null }]);
  assert.equal(canRecuse(recused, [recused]), false);
  assert.equal(canDecide(recused), false);
  assert.equal(disputeState(recused, [recused]), "recused");
  assert.equal(caseFeeText(recused.fee), "Recused · 2,500 sats paid back in efefefefefef…");
  assert.deepEqual(parseRefundQuote({ quote: "q", filer: "buyer", amountSats: 2500, address: "tb1p", feeSats: 200, feeCapSats: 300, totalSats: 2700, expiresInSecs: 120 }), { quote: "q", filer: "buyer", amountSats: 2500, address: "tb1p", feeSats: 200, feeCapSats: 300, totalSats: 2700, expiresInSecs: 120 });
  assert.throws(() => parseRefundQuote({ quote: "q", filer: "moderator", amountSats: 1, feeSats: 1, feeCapSats: 1 }));
});

test("payments a case does not keep are listed as owed back until sent", () => {
  const payment = `${"aa".repeat(32)}:0`;
  const parse = (extra: Record<string, unknown>) => parseDisputes({ disputes: [{ ...letter("buyer"), fee: { state: "expired", amountSats: 2500, dueHeight: 160600, overdue: false, ...extra } }] })[0].fee;
  const owing = parse({ owed: [{ filer: "buyer", amountSats: 2500, payment }, { filer: "buyer", amountSats: 1 }] });
  assert.deepEqual(owing?.owed, [{ filer: "buyer", amountSats: 2500, payment }]);
  assert.equal(caseFeeText(owing), "Filing fee 2,500 sats · request lapsed; a new one goes out while there is time · 2,500 sats owed back");
  const sent = parse({ owed: [], refunds: [{ filer: "buyer", amountSats: 2500, txid: "ef".repeat(32), owed: payment }] });
  assert.equal(sent?.refunds[0].owed, payment);
  assert.doesNotMatch(caseFeeText(sent), /owed/);
});

const lookedUp = () => ({
  provider: "aftership",
  carrier: "usps",
  trackingNumber: "9400100000",
  status: "delivered",
  label: "Delivered",
  deliveredAt: "2026-11-13T15:04:05Z",
  updatedAt: 99,
  events: [{ at: "2026-11-13T15:04:05Z", status: "delivered", label: "Delivered", city: "Portland", state: "OR", country: "United States" }],
  shipTo: { city: "Portland", state: "Oregon", country: "" },
  locationCheck: { verdict: "match", reason: "The delivered checkpoint names the expected locality.", deliveredCity: "Portland", deliveredState: "OR", deliveredCountry: "United States" },
});

test("a dispute shipment lookup parses with its verdict", () => {
  const tracking = parseDisputeTracking(lookedUp());
  assert.equal(tracking?.status, "delivered");
  assert.equal(tracking?.trackingNumber, "9400100000");
  assert.equal(tracking?.shipTo?.city, "Portland");
  assert.equal(tracking?.locationCheck?.verdict, "match");
  const [dispute] = parseDisputes({ disputes: [{ ...letter("buyer"), tracking: lookedUp() }] });
  assert.equal(dispute.tracking?.locationCheck?.verdict, "match");
});

test("an invented verdict or lookup shape is dropped, not shown", () => {
  assert.equal(parseDisputeTracking(null), null);
  assert.equal(parseDisputeTracking("delivered"), null);
  const invented = { ...lookedUp(), locationCheck: { verdict: "trustworthy", reason: "just believe it" } };
  assert.equal(parseDisputeTracking(invented)?.locationCheck, null);
  const noCheck = parseDisputeTracking({ ...lookedUp(), shipTo: null, locationCheck: null });
  assert.equal(noCheck?.locationCheck, null);
  assert.equal(noCheck?.shipTo, null);
});

test("an unreadable desk is an error, not an empty desk", () => {
  assert.throws(() => parseDisputes({}));
  assert.throws(() => parseDisputes({ disputes: [letter("moderator")] }));
  assert.throws(() => parseDisputes({ disputes: [{ favors: "buyer" }] }));
});

test("one ruling closes both letters about an escrow", () => {
  const all = parseDisputes({ disputes: [letter("buyer"), letter("vendor", true)] });
  assert.equal(disputeState(all[0], all), "lost");
  assert.equal(disputeState(all[1], all), "granted");
  const open = parseDisputes({ disputes: [letter("buyer"), letter("vendor")] });
  assert.deepEqual(open.map(dispute => disputeState(dispute, open)), ["open", "open"]);
});

test("the vendor is the seller in the app's words", () => {
  assert.equal(partyName("vendor"), "seller");
  assert.equal(partyName("buyer"), "buyer");
});
