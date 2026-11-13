import assert from "node:assert/strict";
import test from "node:test";
import { parseShipmentTracking } from "./shipment.ts";

test("shipment tracking keeps normalized events and drops unrecognized statuses", () => {
  const tracking = parseShipmentTracking({
    status: "delivered",
    deliveredAt: "2026-11-13T15:04:05Z",
    updatedAt: 99,
    events: [
      { at: "2026-11-12T10:00:00Z", status: "in_transit", city: "Portland", state: "OR", country: "United States" },
      { at: "2026-11-13T15:04:05Z", status: "teleported", city: "Salem", state: "OR", country: "United States" },
    ],
  });
  assert.equal(tracking?.label, "Delivered");
  assert.equal(tracking?.deliveredAt, "2026-11-13T15:04:05Z");
  assert.equal(tracking?.events[1].status, "unknown");
});

test("shipment tracking refuses records without an object snapshot", () => {
  assert.equal(parseShipmentTracking(null), null);
  assert.equal(parseShipmentTracking("delivered"), null);
});
