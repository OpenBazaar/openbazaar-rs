import assert from "node:assert/strict";
import test from "node:test";
import { parseBuyerOrder } from "./catalog.ts";
import { parseReturnCase, parseReturnPolicy, RETURN_STATES, returnStateLabel } from "./returnCase.ts";
import { parseSale } from "./seller.ts";

test("a return is requested, accepted, declined, or shipped", () => {
  assert.deepEqual(RETURN_STATES, ["requested", "accepted", "declined", "shipped"]);
  const opened = parseReturnCase({
    state: "requested",
    reason: "The glaze is crazed.",
    requestedAt: 10,
  });
  assert.equal(opened?.state, "requested");
  assert.equal(returnStateLabel("requested"), "Return requested");
  assert.equal(returnStateLabel("accepted"), "Return accepted");
  assert.equal(returnStateLabel("declined"), "Return declined");
  assert.equal(returnStateLabel("shipped"), "Return shipped");

  const shipped = parseReturnCase({
    state: "shipped",
    reason: "The glaze is crazed.",
    decision: "Send it back.",
    shipment: { carrier: "USPS", tracking: "9400111100000000000000", note: "Packed.", at: 30 },
  });
  assert.equal(shipped?.shipment?.tracking, "9400111100000000000000");
  assert.equal(shipped?.decision, "Send it back.");
});

test("a bad return case is dropped and does not blank the order", () => {
  assert.equal(parseReturnCase(null), null);
  assert.equal(parseReturnCase("requested"), null);
  assert.equal(parseReturnCase({ state: "refunded", reason: "give me the coins" }), null);
  assert.equal(parseReturnCase({ state: "shipped", shipment: "not-a-shipment" })?.shipment, null);
  assert.equal(parseReturnPolicy(null), "");
  assert.equal(parseReturnPolicy("  30 days \u0007 "), "30 days");
  assert.equal(parseReturnPolicy("x".repeat(1200)).length, 1000);

  const sale = parseSale({
    id: "ab12",
    status: "Shipped",
    totalSats: 28000,
    lines: [{ title: "Mug", kind: "physical", quantity: 1, priceSats: 25000 }],
    returnPolicy: "Unused goods can come back within 30 days.",
    returnCase: { state: "refund the escrow" },
  });
  assert.equal(sale.status, "Shipped");
  assert.equal(sale.returnCase, null);
  assert.equal(sale.returnPolicy, "Unused goods can come back within 30 days.");

  const order = parseBuyerOrder({
    id: "order",
    created: 1,
    seller: "Brook and Pine",
    storeId: "ab".repeat(32),
    totalSats: 28000,
    settlement: "direct",
    status: "Shipped",
    remote: { delivered: true, returnCase: { state: "accepted", reason: "Cracked." }, returnPolicy: "30 days" },
  });
  assert.equal(order.status, "Shipped");
  assert.equal(order.remote?.returnCase?.state, "accepted");
  assert.equal(order.remote?.returnPolicy, "30 days");
  assert.equal(order.totalSats, 28000);
});
