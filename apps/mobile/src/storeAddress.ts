// A store's address is the base URL of the node that sells it. A node follows
// `http://` plus a v3 onion, through its own Tor, or a loopback `http://`
// address on the same machine, and nothing else (crates/node/src/peer_http.rs).
// `POST /follows` answers with the follow list even when the store could not
// be fetched: the node keeps the address, tries again each round, and puts
// the reason on the row.
import { NODE_ORIGIN, paired } from "./catalog.ts";
import { parseFollows, type Follow } from "./seller.ts";

const ONION = /^[a-z2-7]{56}\.onion$/;
const LOOPBACK = /^(127\.0\.0\.1|localhost|\[::1\]):\d{1,5}$/;

export const NOT_AN_ADDRESS = "That is not a store address. Paste the address from the seller's storefront: http:// and a name that ends in .onion.";

/** The URL a node follows for what was pasted, or null when it is not a store address. */
export function storeAddress(pasted: string): string | null {
  const host = pasted.trim().replace(/^http:\/\//i, "").replace(/\/+$/, "");
  const onion = host.toLowerCase().replace(/:80$/, "");
  if (ONION.test(onion)) return `http://${onion}`;
  return LOOPBACK.test(host) ? `http://${host}` : null;
}

const change = (method: "POST" | "DELETE", url: string): RequestInit => ({ method, headers: { "Content-Type": "application/json" }, body: JSON.stringify({ url }) });

export async function fetchAddressFollows(): Promise<Follow[]> {
  return parseFollows(await paired("/follows"));
}

export async function followAddress(url: string): Promise<Follow[]> {
  return parseFollows(await paired("/follows", change("POST", url)));
}

export async function unfollowAddress(url: string): Promise<Follow[]> {
  return parseFollows(await paired("/follows", change("DELETE", url)));
}

/** The node's answer when it will not take an address at all. A name that only looks like an onion gets here. */
export function refusal(message: string): string {
  return message.includes("is not a loopback address or a Tor onion") ? "That is not a real store address. Check that it was copied whole." : message;
}

/** Why the node has no copy of a followed store, from the reason it gave. */
export function followProblem(reason: string): string {
  if (reason.includes("not selling")) return "That node answered, but it has no open store yet. Your node keeps the address and checks again.";
  if (reason.includes("not on Tor")) return "Your node is not on Tor, so it cannot reach an onion address. Start it with --remote.";
  if (/connection|timed out|did not answer/i.test(reason)) return "Your node could not reach that store. It may be offline. Your node keeps the address and tries again.";
  return `Your node could not copy that store yet. It keeps the address and tries again. The node said: ${reason}`;
}

/** What following `url` did, read from the list the node sent back. */
export function followOutcome(follows: readonly Follow[], url: string): { ok: boolean; message: string } {
  const follow = follows.find(item => item.url === url);
  if (follow?.storeId && !follow.error) return { ok: true, message: `Following ${follow.name ?? "that store"}. Its listings are in Products.` };
  return { ok: false, message: followProblem(follow?.error ?? "it has not fetched the store") };
}

/**
 * Where another node follows this node's store. `onion` is the public store
 * address, there once a node started with `--remote` has published it.
 * `local` works only for a node on this same computer.
 */
export type OwnAddress = { onion: string | null; onTor: boolean; local: string | null };

/** `remote` is the node's `GET /remote`; `origin` is the address this page calls the node at. */
export function ownAddress(remote: unknown, origin: string): OwnAddress {
  const body = remote && typeof remote === "object" ? (remote as { saleOnion?: unknown; tor?: unknown }) : {};
  const onion = typeof body.saleOnion === "string" ? storeAddress(body.saleOnion) : null;
  const local = storeAddress(origin);
  return {
    onion: onion?.endsWith(".onion") ? onion : null,
    onTor: !!body.tor && typeof body.tor === "object",
    local: local && !local.endsWith(".onion") ? local : null,
  };
}

export async function fetchOwnAddress(): Promise<OwnAddress> {
  return ownAddress(await paired("/remote"), NODE_ORIGIN);
}
