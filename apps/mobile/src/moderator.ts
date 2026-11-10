import { openJson, paired } from "./catalog.ts";

// A moderator node's desk: the dispute letters it has checked and the
// rulings its operator gives (`/moderator/disputes` on the node).

export type Party = "buyer" | "vendor";

export type Dispute = {
  /** The escrow the letter is about. Both parties can write about one escrow. */
  id: string;
  /** Who wrote the letter. A letter can only be granted to its writer. */
  favors: Party;
  memo: string;
  outpoint: string;
  receivedHeight: number;
  ruled: boolean;
  /** The escrow names the built-in test moderator, not this node's own key. */
  builtIn: boolean;
};

/** What the operator can still do with `dispute`, given every letter on the desk. */
export type DisputeState = "open" | "granted" | "lost";

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
      builtIn: r.builtIn === true,
    };
  });
}

/** An escrow takes one ruling: once one letter is granted, the other party's letter has lost. */
export function disputeState(dispute: Dispute, all: Dispute[]): DisputeState {
  if (dispute.ruled) return "granted";
  return all.some(other => other.id === dispute.id && other.ruled) ? "lost" : "open";
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

/** Grant `dispute` to the party that wrote it. The node's wallet must be unlocked. */
export async function ruleDispute(dispute: Dispute): Promise<void> {
  await paired(`/moderator/disputes/${dispute.id}/rule`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ favor: dispute.favors }),
  });
}
