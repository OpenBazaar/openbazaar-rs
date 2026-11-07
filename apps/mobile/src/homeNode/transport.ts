import { bytesToHex, hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";
import { accessKey, deviceKey, newDeviceKey, pairDigest, parsePairingCode, sign, signedHeaders, type DeviceKey } from "./signing.ts";

// Where the shop's node requests go. There are three cases:
//
// - The page is served by a home node to a remote browser (Tor Browser at the
//   node's onion). index.html then carries <meta name="ob-home-node">; every
//   request goes to this origin, signed with this browser's device key.
// - The desktop app with its target set to "home": requests go to the app's
//   own node under /home/proxy, which signs them and carries them over Tor.
// - Otherwise: the app's own node, as before.

export type NodeTarget = "local" | "home";
const TARGET_KEY = "ob-node-target";
const DEVICE_KEY = "ob-home-device";

export class HomeSignInRequired extends Error {
  constructor() {
    super("Pair this browser with your home node, or sign in with its access key.");
    this.name = "HomeSignInRequired";
  }
}

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function servedByHomeNode(): boolean {
  return typeof document !== "undefined" && document.querySelector('meta[name="ob-home-node"]') !== null;
}

export function nodeTarget(): NodeTarget {
  return !servedByHomeNode() && storage()?.getItem(TARGET_KEY) === "home" ? "home" : "local";
}

/** Switch the app between its own node and the paired home node, then reload so every screen asks the new node. */
export function switchTarget(target: NodeTarget): void {
  if (target === "home") storage()?.setItem(TARGET_KEY, "home");
  else storage()?.removeItem(TARGET_KEY);
  if (typeof location !== "undefined") location.reload();
}

/** Prefix for the app's own node routes when the home node is the target. */
export function targetPrefix(): string {
  return nodeTarget() === "home" ? "/home/proxy" : "";
}

type Session = { key: DeviceKey; nodeId: string };
let session: Session | null = null;
let clockOffset = 0;

export function hasSession(): boolean {
  return session !== null;
}

function now(): number {
  return Math.floor(Date.now() / 1000) + clockOffset;
}

async function hello(): Promise<string> {
  const response = await fetch("/remote/hello", { cache: "no-store" });
  const body = (await response.json()) as { nodeId?: unknown };
  if (typeof body.nodeId !== "string") throw new Error("This page is not served by an OpenBazaar home node.");
  return body.nodeId;
}

function bodyBytes(body: BodyInit | null | undefined): Uint8Array {
  if (body === undefined || body === null) return new Uint8Array();
  if (typeof body === "string") return utf8ToBytes(body);
  throw new Error("Only text bodies can be sent to a home node.");
}

/** fetch() for a page served by the home node: signed, with one retry on the node's clock. */
export async function signedFetch(path: string, init: RequestInit = {}): Promise<Response> {
  if (!session) throw new HomeSignInRequired();
  const method = (init.method ?? "GET").toUpperCase();
  const body = bodyBytes(init.body);
  for (let attempt = 0; ; attempt++) {
    const headers = { ...(init.headers as Record<string, string> | undefined), ...signedHeaders(session.key, session.nodeId, now(), method, path, body) };
    const response = await fetch(path, { ...init, method, headers });
    if (response.status !== 401 || attempt > 0) return response;
    const detail = (await response.clone().json().catch(() => ({}))) as { code?: unknown; serverTime?: unknown };
    if (detail.code === "clock" && typeof detail.serverTime === "number") {
      clockOffset = detail.serverTime - Math.floor(Date.now() / 1000);
      continue;
    }
    if (detail.code === "unknown_device") signOut();
    return response;
  }
}

/** Sign in with a device secret: checks it against the node, and keeps it in this tab (and this browser, if asked). */
export async function signIn(secret: Uint8Array, remember: boolean): Promise<void> {
  let key: DeviceKey;
  try {
    key = deviceKey(secret);
  } catch {
    throw new Error("That access key is not valid. Check it against the one you saved.");
  }
  const candidate = { key, nodeId: await hello() };
  session = candidate;
  const check = await signedFetch("/remote/devices");
  if (!check.ok) {
    session = null;
    const body = (await check.json().catch(() => ({}))) as { error?: unknown };
    throw new Error(typeof body.error === "string" ? body.error : "The node did not accept this access key.");
  }
  session = candidate;
  if (remember) storage()?.setItem(DEVICE_KEY, bytesToHex(secret));
  else storage()?.removeItem(DEVICE_KEY);
}

/** Use a key this browser remembered, if any. */
export async function restoreSession(): Promise<boolean> {
  const stored = storage()?.getItem(DEVICE_KEY);
  if (!stored || !/^[0-9a-f]{64}$/.test(stored)) return false;
  try {
    await signIn(hexToBytes(stored), true);
    return true;
  } catch {
    return false;
  }
}

export function signOut(): void {
  session = null;
  storage()?.removeItem(DEVICE_KEY);
}

/** Pair this browser with the node that served it. Returns the access key to save. */
export async function pairBrowser(code: string, name: string, remember: boolean): Promise<string> {
  const parsed = parsePairingCode(code);
  if (!parsed) throw new Error("That is not a pairing code. It starts with obhome:.");
  const key = newDeviceKey();
  const response = await fetch("/remote/pair", {
    method: "POST",
    cache: "no-store",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ secret: parsed.secret, publicKey: key.publicKey, name, signature: sign(key, pairDigest(parsed.secret, key.publicKey, name)) }),
  });
  const body = (await response.json().catch(() => ({}))) as { error?: unknown; deviceId?: unknown };
  if (!response.ok || body.deviceId !== key.id) {
    throw new Error(typeof body.error === "string" ? body.error : "The node refused the pairing code.");
  }
  await signIn(key.secret, remember);
  return accessKey(key.secret);
}
