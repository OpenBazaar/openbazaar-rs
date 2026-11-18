import { openJson, paired } from "./catalog.ts";
import { parseShipmentTracking, type ShipmentTracking } from "./shipment.ts";

// A moderator node's desk: the dispute letters it has checked and the
// rulings its operator gives (`/moderator/disputes` on the node).

export type Party = "buyer" | "vendor";

export type LocationVerdict = "match" | "mismatch" | "unverifiable";

/** How the carrier's delivered place compares with the locality the operator expected. Advisory. */
export type LocationCheck = {
  verdict: LocationVerdict;
  reason: string;
  deliveredCity: string;
  deliveredState: string;
  deliveredCountry: string;
};

/** This moderator node's own carrier lookup for a disputed shipment. */
export type DisputeTracking = ShipmentTracking & {
  carrier: string;
  trackingNumber: string;
  shipTo: { city: string; state: string; country: string } | null;
  locationCheck: LocationCheck | null;
};

export type CaseFeeState = "free" | "awaiting-request" | "requested" | "expired" | "paid";

/** The filing fee on a v5 case, as this moderator's node sees it. */
export type CaseFee = {
  state: CaseFeeState;
  amountSats: number;
  /** The decision is due by this block; a paid case past it is overdue. */
  dueHeight: number;
  paidBy: Party | null;
  rail: "ln" | "onchain" | null;
  overdue: boolean;
  /** This node recused itself from the case and paid the fee back. */
  recused: boolean;
  /** Payments this node sent back from its wallet. */
  refunds: CaseRefund[];
};

export type CaseRefund = { filer: Party; amountSats: number; txid: string };

/** A refund from this node's wallet, as the operator is asked to confirm it. */
export type RefundQuote = {
  quote: string;
  filer: Party;
  amountSats: number;
  address: string | null;
  /** The network fee this node pays on top, and the most it may become. */
  feeSats: number;
  feeCapSats: number;
  totalSats: number;
  expiresInSecs: number;
};

export type Dispute = {
  /** The escrow the letter is about. Both parties can write about one escrow. */
  id: string;
  /** Who wrote the letter. A letter can only be granted to its writer. */
  favors: Party;
  memo: string;
  outpoint: string;
  receivedHeight: number;
  ruled: boolean;
  /** Denied: final, and the filing fee is kept. */
  denied: boolean;
  /** The escrow names the built-in test moderator, not this node's own key. */
  builtIn: boolean;
  /** The shipment lookup this node already ran for the escrow, if any. */
  tracking: DisputeTracking | null;
  /** `null` for an escrow from before filing fees. */
  fee: CaseFee | null;
};

/** What the operator can still do with `dispute`, given every letter on the desk. */
export type DisputeState = "open" | "granted" | "denied" | "lost" | "recused";

const FEE_STATES: readonly CaseFeeState[] = ["free", "awaiting-request", "requested", "expired", "paid"];

export function parseCaseFee(value: unknown): CaseFee | null {
  const r = value && typeof value === "object" ? (value as Record<string, unknown>) : null;
  const state = FEE_STATES.find(known => known === r?.state);
  if (!r || !state) return null;
  const count = (n: unknown) => (typeof n === "number" && Number.isSafeInteger(n) && n >= 0 ? n : 0);
  return {
    state,
    amountSats: count(r.amountSats),
    dueHeight: count(r.dueHeight),
    paidBy: r.paidBy === "buyer" || r.paidBy === "vendor" ? r.paidBy : null,
    rail: r.rail === "ln" || r.rail === "onchain" ? r.rail : null,
    overdue: r.overdue === true,
    recused: r.recused === true,
    refunds: Array.isArray(r.refunds) ? r.refunds.flatMap(item => {
      const refund = item && typeof item === "object" ? (item as Record<string, unknown>) : {};
      const filer = refund.filer === "buyer" || refund.filer === "vendor" ? refund.filer : null;
      return filer && typeof refund.txid === "string" ? [{ filer, amountSats: count(refund.amountSats), txid: refund.txid }] : [];
    }) : [],
  };
}

export function parseRefundQuote(value: unknown): RefundQuote {
  const r = value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  const count = (n: unknown) => (typeof n === "number" && Number.isSafeInteger(n) && n >= 0 ? n : null);
  const amountSats = count(r.amountSats), feeSats = count(r.feeSats), feeCapSats = count(r.feeCapSats);
  if (typeof r.quote !== "string" || !r.quote || (r.filer !== "buyer" && r.filer !== "vendor") || amountSats === null || feeSats === null || feeCapSats === null) {
    throw new Error("The node returned an unreadable refund quote.");
  }
  return { quote: r.quote, filer: r.filer, amountSats, address: typeof r.address === "string" ? r.address : null, feeSats, feeCapSats, totalSats: count(r.totalSats) ?? amountSats + feeSats, expiresInSecs: count(r.expiresInSecs) ?? 0 };
}

export function parseDisputes(value: unknown): Dispute[] {
  const list = value && typeof value === "object" ? (value as Record<string, unknown>).disputes : null;
  if (!Array.isArray(list)) throw new Error("The node returned an unreadable list of disputes.");
  return list.map(item => {
    const r = (item && typeof item === "object" ? item : {}) as Record<string, unknown>;
    if (typeof r.id !== "string" || !r.id || (r.favors !== "buyer" && r.favors !== "vendor")) {
      throw new Error("The node returned an unreadable dispute.");
    }
    return {
      id: r.id,
      favors: r.favors,
      memo: typeof r.memo === "string" ? r.memo : "",
      outpoint: typeof r.outpoint === "string" ? r.outpoint : "",
      receivedHeight: typeof r.receivedHeight === "number" ? r.receivedHeight : 0,
      ruled: r.ruled === true,
      denied: r.denied === true,
      builtIn: r.builtIn === true,
      tracking: parseDisputeTracking(r.tracking),
      fee: parseCaseFee(r.fee),
    };
  });
}

const bounded = (value: unknown, max = 60) => (typeof value === "string" ? value.slice(0, max) : "");

/** Parse a dispute lookup without letting a node invent verdicts or events. */
export function parseDisputeTracking(value: unknown): DisputeTracking | null {
  const base = parseShipmentTracking(value);
  if (!base) return null;
  const r = value as Record<string, unknown>;
  const shipTo = r.shipTo && typeof r.shipTo === "object" ? (r.shipTo as Record<string, unknown>) : null;
  const check = r.locationCheck && typeof r.locationCheck === "object" ? (r.locationCheck as Record<string, unknown>) : null;
  const verdict = check?.verdict;
  return {
    ...base,
    carrier: bounded(r.carrier),
    trackingNumber: bounded(r.trackingNumber, 64),
    shipTo: shipTo ? { city: bounded(shipTo.city), state: bounded(shipTo.state), country: bounded(shipTo.country) } : null,
    locationCheck: check && (verdict === "match" || verdict === "mismatch" || verdict === "unverifiable") ? {
      verdict,
      reason: bounded(check.reason, 200),
      deliveredCity: bounded(check.deliveredCity),
      deliveredState: bounded(check.deliveredState),
      deliveredCountry: bounded(check.deliveredCountry),
    } : null,
  };
}

/** What the operator copies out of the dispute memos for the node to verify. */
export type ShipmentLookup = {
  carrier: string;
  trackingNumber: string;
  shipTo?: { city: string; state?: string; country?: string };
};

/**
 * Ask this moderator node to look the disputed shipment up first-hand. The
 * node contacts the tracking provider itself, through Tor, so the answer is
 * evidence the moderator observed rather than a party's claim.
 */
export async function lookUpDisputeShipment(id: string, lookup: ShipmentLookup): Promise<DisputeTracking | null> {
  const answer = await paired(`/moderator/disputes/${id}/tracking`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(lookup),
  });
  return parseDisputeTracking(answer);
}

/** An escrow takes one ruling: once one letter is granted, the other party's letter has lost. */
export function disputeState(dispute: Dispute, all: Dispute[]): DisputeState {
  if (dispute.ruled) return "granted";
  if (dispute.fee?.recused) return "recused";
  if (dispute.denied) return "denied";
  return all.some(other => other.id === dispute.id && other.ruled) ? "lost" : "open";
}

/** The operator may decide once the filing fee is paid, or when there is none. */
export function canDecide(dispute: Dispute): boolean {
  return !dispute.fee || dispute.fee.state === "free" || (dispute.fee.state === "paid" && !dispute.fee.recused);
}

/** The operator may recuse from a paid case until any letter of it is decided. */
export function canRecuse(dispute: Dispute, all: Dispute[]): boolean {
  return dispute.fee?.state === "paid" && !dispute.fee.recused && !all.some(other => other.id === dispute.id && (other.ruled || other.denied));
}

/** The filing fee on a letter in words, or "" for an escrow from before filing fees. */
export function caseFeeText(fee: CaseFee | null): string {
  if (!fee) return "";
  const sats = `${fee.amountSats.toLocaleString()} sats`;
  const due = `decision due by block ${fee.dueHeight.toLocaleString()}`;
  if (fee.recused) {
    const back = fee.refunds.find(refund => refund.txid);
    return `Recused · ${sats} paid back${back ? ` in ${back.txid.slice(0, 12)}…` : ""}`;
  }
  switch (fee.state) {
    case "free": return `No filing fee · ${due}`;
    case "awaiting-request": return `Filing fee ${sats} · request not sent yet`;
    case "requested": return `Filing fee ${sats} · awaiting payment`;
    case "expired": return `Filing fee ${sats} · request lapsed; a new one goes out while there is time`;
    case "paid": return fee.overdue ? `Paid ${sats} · overdue: block ${fee.dueHeight.toLocaleString()} has passed` : `Paid ${sats}${fee.rail ? fee.rail === "ln" ? " over Lightning" : " on-chain" : ""} · ${due}`;
  }
}

export const partyName = (party: Party) => (party === "vendor" ? "seller" : "buyer");

/** True when the node in use runs a moderator's desk. `/network` is open, so asking never needs a pairing code. */
export async function nodeModerates(): Promise<boolean> {
  const network = await openJson("/network");
  return !!network && typeof network === "object" && (network as Record<string, unknown>).moderates === true;
}

export async function fetchDisputes(): Promise<Dispute[]> {
  return parseDisputes(await paired("/moderator/disputes"));
}

/** Deny `dispute`: final, and the filing fee is kept. The node's wallet must be unlocked. */
export async function denyDispute(dispute: Dispute): Promise<void> {
  await paired(`/moderator/disputes/${dispute.id}/deny`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ favor: dispute.favors }),
  });
}

const post = (path: string, body: object = {}) =>
  paired(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });

/** What recusing from `dispute`'s case pays back, to confirm with the passphrase. */
export async function quoteRecusal(dispute: Dispute): Promise<RefundQuote> {
  return parseRefundQuote(await post(`/moderator/disputes/${dispute.id}/recuse/quote`));
}

/** Recuse from the case and pay the fee back. Final. Returns the refund's txid. */
export async function recuseDispute(dispute: Dispute, quote: string, passphrase: string): Promise<string> {
  const answer = await post(`/moderator/disputes/${dispute.id}/recuse`, { quote, passphrase, confirm: true });
  const txid = answer && typeof answer === "object" ? (answer as Record<string, unknown>).txid : null;
  if (typeof txid !== "string") throw new Error("The node returned an unreadable refund.");
  return txid;
}

/** Grant `dispute` to the party that wrote it. The node's wallet must be unlocked. */
export async function ruleDispute(dispute: Dispute): Promise<void> {
  await paired(`/moderator/disputes/${dispute.id}/rule`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ favor: dispute.favors }),
  });
}
