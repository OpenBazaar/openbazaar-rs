import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";

import { LAN_HOST, jsonBody, jsonOf, lanExchange, probeLan, type Channels, type NodeLink, type Via } from "./client.ts";
import { decodeResponse, encodeRequest, type RawResponse } from "./http1.ts";
import { deviceIdOf, nodeIdOf, pairingOnion, pairProof, sas, transcript, type PairRequestFields } from "./protocol.ts";

// The device side of pairing: POST /pair/request on the LAN channel (when a
// QR gave LAN addresses and the node's key) or on the pairing onion, then
// long-poll GET /pair/status/{requestId} until the owner decides on the node.

export type PairingCode = { secret: Uint8Array; nodePublic?: Uint8Array; onion?: string; lan: string[] };

export type PairingRequest = PairRequestFields & { sign(text: string): Promise<string> };

export type PendingPairing = {
  requestId: string;
  nodePublic: Uint8Array;
  sas: { emoji: string; name: string }[];
  expiresInSecs: number;
  route: PairRoute;
};

type PairRoute = { via: "lan"; address: string; nodePublic: Uint8Array; host: string } | { via: "tor"; onion: string };

export type PairingResult =
  | { state: "pending" | "rejected" | "expired" }
  | { state: "approved"; link: NodeLink; discoverySecret: string | null };

const POLL_TIMEOUT_MS = 40_000;
const REQUEST_TIMEOUT_MS = 120_000;

/** How long to keep asking a pairing onion that Tor cannot reach yet, the pause between tries, and who hears why a try ended. */
export type Patience = { forMs: number; pauseMs: number; retrying?: (reason: string) => void };
const PATIENCE: Patience = { forMs: 5 * 60_000, pauseMs: 3000 };

export class PairingError extends Error {
  readonly code?: string;
  constructor(message: string, code?: string) {
    super(message);
    this.name = "PairingError";
    this.code = code;
  }
}

async function call(channels: Channels, route: PairRoute, method: string, path: string, body: Uint8Array, timeoutMs: number): Promise<RawResponse> {
  const headers: Record<string, string> = body.length ? { "Content-Type": "application/json" } : {};
  if (route.via === "lan") {
    return lanExchange(channels, route.address, route.nodePublic, encodeRequest(method, path, route.host, headers, body), timeoutMs);
  }
  return decodeResponse(await channels.tor(route.onion, encodeRequest(method, path, `${route.onion}.onion`, headers, body), timeoutMs));
}

const REFUSALS: Record<string, string> = {
  code_taken: "Another device already used this code. Make a new one on your node.",
  paused: "Too many wrong codes. Pairing is paused for a few minutes.",
};

// A code's pairing onion is new: for its first minute or two some Tor
// directories do not know it yet, and a Tor client that has just started can
// spend a whole attempt on circuits that stall. One failed attempt says
// little, so ask again until the node answers or the patience runs out.
async function ask(channels: Channels, route: PairRoute, body: Uint8Array, patience: Patience): Promise<RawResponse> {
  const until = Date.now() + patience.forMs;
  for (;;) {
    try {
      return await call(channels, route, "POST", "/pair/request", body, REQUEST_TIMEOUT_MS);
    } catch (err) {
      if (route.via !== "tor") throw err;
      const reason = err instanceof Error && err.message ? err.message : "no answer";
      if (Date.now() >= until) {
        throw new PairingError(`Your node did not answer over Tor (${reason}). Check that the code is still showing on your node and that this phone is online, then try again.`);
      }
      patience.retrying?.(reason);
      await new Promise(resolve => setTimeout(resolve, patience.pauseMs));
    }
  }
}

export async function requestPairing(channels: Channels, code: PairingCode, request: PairingRequest, patience: Partial<Patience> = {}): Promise<PendingPairing> {
  const text = transcript(request);
  const body = jsonBody({
    v: 2,
    key: request.key,
    ...(request.watchKey ? { watchKey: request.watchKey } : {}),
    ...(request.discoveryKey ? { discoveryKey: request.discoveryKey } : {}),
    name: request.name,
    platform: request.platform,
    proof: pairProof(code.secret, text),
    signature: await request.sign(text),
  });
  let route: PairRoute = { via: "tor", onion: pairingOnion(code.secret) };
  if (code.nodePublic && code.onion && code.lan.length) {
    const address = await probeLan(channels, code.lan, code.nodePublic);
    if (address) route = { via: "lan", address, nodePublic: code.nodePublic, host: LAN_HOST };
  }
  const answer = await ask(channels, route, body, { ...PATIENCE, ...patience });
  const value = jsonOf(answer.body);
  if (answer.status !== 202) {
    const known = typeof value.code === "string" ? REFUSALS[value.code] : undefined;
    throw new PairingError(known ?? (typeof value.error === "string" ? value.error : `The node refused the request (${answer.status}).`), value.code as string | undefined);
  }
  if (typeof value.requestId !== "string" || typeof value.nodeKey !== "string" || !/^[0-9a-f]{64}$/.test(value.nodeKey)) {
    throw new PairingError("The node sent an unreadable answer.");
  }
  const nodePublic = hexToBytes(value.nodeKey);
  if (code.nodePublic && bytesToHex(code.nodePublic) !== value.nodeKey) {
    throw new PairingError("The node that answered is not the one in the QR code. Do not continue.");
  }
  const pictures = sas(code.secret, nodePublic, text);
  const names = Array.isArray(value.sasNames) ? value.sasNames : [];
  if (names.length && names.join(" ") !== pictures.map(p => p.name).join(" ")) {
    throw new PairingError("The node's pictures do not match this phone's. Do not continue.");
  }
  return {
    requestId: value.requestId,
    nodePublic,
    sas: pictures,
    expiresInSecs: typeof value.expiresInSecs === "number" ? value.expiresInSecs : 600,
    route,
  };
}

/** One long poll (up to 25 s at the node). */
export async function pollPairing(channels: Channels, pending: PendingPairing, keyText: string): Promise<PairingResult> {
  const answer = await call(channels, pending.route, "GET", `/pair/status/${pending.requestId}`, new Uint8Array(), POLL_TIMEOUT_MS);
  const value = jsonOf(answer.body);
  if (answer.status === 404) return { state: "expired" };
  if (answer.status !== 200) throw new PairingError(typeof value.error === "string" ? value.error : `The node answered ${answer.status}.`);
  if (value.state === "pending" || value.state === "rejected" || value.state === "expired") return { state: value.state };
  if (value.state !== "approved") throw new PairingError("The node sent an unreadable answer.");
  const onion = typeof value.onion === "string" ? value.onion.toLowerCase().replace(/\.onion$/, "") : "";
  const nodeId = nodeIdOf(pending.nodePublic);
  const deviceId = deviceIdOf(keyText);
  if (value.nodeId !== nodeId || value.deviceId !== deviceId || !/^[a-z2-7]{56}$/.test(onion)) {
    throw new PairingError("The node approved a different device or sent a mismatched id. Pair again.");
  }
  const lan = Array.isArray(value.lan) ? value.lan.filter((item): item is string => typeof item === "string") : [];
  return {
    state: "approved",
    link: { nodeId, nodeKey: bytesToHex(pending.nodePublic), onion, lan, deviceId },
    discoverySecret: typeof value.discoverySecret === "string" ? value.discoverySecret : null,
  };
}

export function pairedVia(pending: PendingPairing): Via {
  return pending.route.via;
}
