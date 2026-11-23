/** A return case on one order. The node is the source of the state. */

export const RETURN_STATES = ["requested", "accepted", "declined", "shipped"] as const;

export type ReturnState = (typeof RETURN_STATES)[number];

export type ReturnShipment = {
  carrier: string;
  tracking: string;
  note: string;
  at: number;
};

export type ReturnCase = {
  state: ReturnState;
  reason: string;
  requestedAt: number;
  decision: string;
  decidedAt: number;
  shipment: ReturnShipment | null;
};

const text = (value: unknown, max: number) => (typeof value === "string" ? value.slice(0, max) : "");
const count = (value: unknown) => (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0);

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function line(value: unknown, max: number): string {
  return text(value, max).replace(/[\u0000-\u001f\u007f]/g, "").trim();
}

/** The policy the order was sold under. A missing or hostile value is no policy. */
export function parseReturnPolicy(value: unknown): string {
  return line(value, 1000);
}

export function returnStateLabel(state: ReturnState): string {
  switch (state) {
    case "requested":
      return "Return requested";
    case "accepted":
      return "Return accepted";
    case "declined":
      return "Return declined";
    case "shipped":
      return "Return shipped";
  }
}

function parseShipment(value: unknown): ReturnShipment | null {
  const shipment = record(value);
  if (!shipment) return null;
  const carrier = line(shipment.carrier, 60);
  const tracking = line(shipment.tracking, 100);
  const note = line(shipment.note, 1000);
  if (!carrier && !tracking && !note) return null;
  return { carrier, tracking, note, at: count(shipment.at) };
}

/**
 * One return, or nothing. An unknown state or a non-object is dropped so a
 * bad case cannot blank the rest of the order.
 */
export function parseReturnCase(value: unknown): ReturnCase | null {
  const item = record(value);
  if (!item) return null;
  const state = item.state;
  if (typeof state !== "string" || !RETURN_STATES.includes(state as ReturnState)) return null;
  return {
    state: state as ReturnState,
    reason: line(item.reason, 2000),
    requestedAt: count(item.requestedAt),
    decision: line(item.decision, 2000),
    decidedAt: count(item.decidedAt),
    shipment: parseShipment(item.shipment),
  };
}
