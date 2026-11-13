/** Normalized shipment tracking shared by a sale and a buyer's order. */

export type ShipmentEvent = {
  at: string;
  status: string;
  label: string;
  city: string;
  state: string;
  country: string;
};

export type ShipmentTracking = {
  status: string;
  label: string;
  deliveredAt: string | null;
  updatedAt: number;
  events: ShipmentEvent[];
};

export const CARRIER_OPTIONS = [
  { value: "USPS", label: "USPS" },
  { value: "UPS", label: "UPS" },
  { value: "FedEx", label: "FedEx" },
  { value: "DHL", label: "DHL" },
  { value: "Other", label: "Other carrier" },
] as const;

const LABELS: Record<string, string> = {
  pending: "Waiting for carrier updates",
  label_created: "Label created",
  in_transit: "In transit",
  out_for_delivery: "Out for delivery",
  delivery_attempted: "Delivery attempted",
  available_for_pickup: "Available for pickup",
  delivered: "Delivered",
  exception: "Delivery exception",
  returned: "Returned to sender",
  expired: "Tracking expired",
  manual: "Tracking entered by the seller",
  unknown: "Tracking update",
};

const text = (value: unknown, max = 200) => (typeof value === "string" ? value.slice(0, max) : "");
const count = (value: unknown) => (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0);

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

export function shipmentLabel(status: string): string {
  return LABELS[status] ?? LABELS.unknown;
}

/** Parse a tracking snapshot without letting a node invent statuses or events. */
export function parseShipmentTracking(value: unknown): ShipmentTracking | null {
  const tracking = record(value);
  if (!tracking) return null;
  const rawStatus = text(tracking.status, 40);
  const status = rawStatus in LABELS ? rawStatus : "unknown";
  const events = (Array.isArray(tracking.events) ? tracking.events : []).flatMap((item) => {
    const event = record(item);
    if (!event || !text(event.at, 40)) return [];
    const eventStatus = text(event.status, 40);
    const normalized = eventStatus in LABELS ? eventStatus : "unknown";
    return [{
      at: text(event.at, 40),
      status: normalized,
      label: text(event.label, 80) || shipmentLabel(normalized),
      city: text(event.city, 60),
      state: text(event.state, 60),
      country: text(event.country, 60),
    }];
  }).slice(-50);
  return {
    status,
    label: text(tracking.label, 80) || shipmentLabel(status),
    deliveredAt: typeof tracking.deliveredAt === "string" ? text(tracking.deliveredAt, 40) : null,
    updatedAt: count(tracking.updatedAt),
    events,
  };
}
