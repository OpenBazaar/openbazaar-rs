/** `tor` in the node's `GET /remote`: how its Tor is doing, in the node's own words. */
export type TorHealth = { state: "starting" | "fine" | "degraded" | "cut off"; reason: string; since: number };

const STATES = ["starting", "fine", "degraded", "cut off"] as const;

/** null for a node that sends no `tor`: an older one, or one without Tor. */
export function parseTorHealth(value: unknown): TorHealth | null {
  if (!value || typeof value !== "object") return null;
  const { state, reason, since } = value as Record<string, unknown>;
  const known = STATES.find(name => name === state);
  if (!known || typeof reason !== "string" || typeof since !== "number") return null;
  return { state: known, reason, since };
}

/** What to tell the owner, or null while Tor is fine or still starting. `when` words a time in unix seconds. */
export function torWarning(health: TorHealth | null, when: (secs: number) => string): string | null {
  if (health?.state === "cut off") return `This node has been cut off from Tor since ${when(health.since)}. ${health.reason} Devices away from home cannot reach it. Check the node's internet connection, then restart the node.`;
  if (health?.state === "degraded") return `This node has had trouble with Tor since ${when(health.since)}. ${health.reason} It often recovers by itself. If it has not within an hour, restart the node.`;
  return null;
}
