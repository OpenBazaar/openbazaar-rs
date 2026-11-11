import { ownNodeJson, paired } from "../catalog.ts";
import { parseTorHealth, type TorHealth } from "./torHealth.ts";

export type RemoteDevice = {
  id: string;
  name: string;
  platform: string;
  pairedAt: number;
  lastSeen: number;
  lastVia: string | null;
  pairedBy: string | null;
  push: boolean;
  thisDevice: boolean;
};

export type RemoteStatus = {
  enabled: boolean;
  onion: string | null;
  onionState: string | null;
  moving: boolean;
  lan: string[];
  devices: number;
  webShop: boolean;
  /** How the node's Tor is doing. null from an older node, or one without Tor. */
  tor: TorHealth | null;
};

export type PendingDevice = { requestId: string; name: string; platform: string; sas: string[]; sasNames: string[] };
/** How the node is doing with a code's own onion address. null: nothing to wait for (the node has no Tor, or is an older one). */
export type PairingOnion = "publishing" | "ready" | "slow" | null;
/** `lan`: the link also carries addresses that work on the node's local network, without Tor. */
export type PairingCode = { words: string[]; link: string; expiresInSecs: number; codeId: string; onion: PairingOnion; lan: boolean };
export type PairingCodeState = { codeId: string; onion: PairingOnion; expiresInSecs: number };
export type BrowserCredential = { accessKey: string; discoverySecret: string; onion: string | null };
export type HomeLink = { paired: false } | { paired: true; onion: string; deviceId: string; name: string; pairedAt: number; via: string | null };
export type HomePairing = { requestId: string; sas: string[]; sasNames: string[] };

const json = { "Content-Type": "application/json" };
const record = (value: unknown): Record<string, unknown> => (value && typeof value === "object" ? (value as Record<string, unknown>) : {});
const text = (value: unknown): string | null => (typeof value === "string" ? value : null);
const count = (value: unknown): number => (typeof value === "number" && Number.isFinite(value) ? value : 0);
const texts = (value: unknown): string[] => (Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : []);
const onionState = (value: unknown): PairingOnion => (value === "publishing" || value === "ready" || value === "slow" ? value : null);

function parseDevices(value: unknown): RemoteDevice[] {
  const list = record(value).devices;
  return (Array.isArray(list) ? list : []).map(record).map(item => ({
    id: text(item.id) ?? "",
    name: text(item.name) ?? "Unnamed device",
    platform: text(item.platform) ?? "desktop",
    pairedAt: count(item.pairedAt),
    lastSeen: count(item.lastSeen),
    lastVia: text(item.lastVia),
    pairedBy: text(item.pairedBy),
    push: record(item.push).enabled === true,
    thisDevice: item.thisDevice === true,
  }));
}

// The node in use: this app's own node, the home node through it, or the
// home node that served this page.

export async function fetchRemoteStatus(): Promise<RemoteStatus> {
  const body = record(await paired("/remote"));
  return {
    enabled: body.enabled === true,
    onion: text(body.onion),
    onionState: text(body.onionState),
    moving: body.moving === true,
    lan: texts(body.lan),
    devices: count(body.devices),
    webShop: body.webShop === true,
    tor: parseTorHealth(body.tor),
  };
}

export async function fetchDevices(): Promise<RemoteDevice[]> {
  return parseDevices(await paired("/remote/devices"));
}

/** Remove a device (and the devices it paired); with `rotate`, also move the node to a new onion address. */
export async function revokeDevice(id: string, rotate: boolean): Promise<RemoteDevice[]> {
  return parseDevices(await paired(`/remote/devices/${encodeURIComponent(id)}`, { method: "DELETE", body: JSON.stringify({ rotate }), headers: json }));
}

export async function createPairingCode(): Promise<PairingCode> {
  const body = record(await paired("/remote/pairing", { method: "POST", body: "{}", headers: json }));
  const link = text(body.link);
  if (!link) throw new Error("The node sent no pairing code.");
  return { words: texts(body.words), link, expiresInSecs: count(body.expiresInSecs), codeId: text(body.codeId) ?? "", onion: onionState(body.onion), lan: body.lan === true };
}

/** The devices asking to pair, and how each open code's onion is doing. */
export async function fetchPairing(): Promise<{ pending: PendingDevice[]; codes: PairingCodeState[] }> {
  const body = record(await paired("/remote/pending"));
  const list = (value: unknown) => (Array.isArray(value) ? value : []).map(record);
  return {
    pending: list(body.pending).map(item => ({
      requestId: text(item.requestId) ?? "",
      name: text(item.name) ?? "Unnamed device",
      platform: text(item.platform) ?? "",
      sas: texts(item.sas),
      sasNames: texts(item.sasNames),
    })),
    codes: list(body.codes).map(item => ({ codeId: text(item.codeId) ?? "", onion: onionState(item.onion), expiresInSecs: count(item.expiresInSecs) })),
  };
}

export async function fetchPending(): Promise<PendingDevice[]> {
  return (await fetchPairing()).pending;
}

export async function decidePending(requestId: string, approve: boolean): Promise<void> {
  await paired(`/remote/pending/${encodeURIComponent(requestId)}`, { method: "POST", body: JSON.stringify({ approve }), headers: json });
}

export async function createBrowserCredential(name: string): Promise<BrowserCredential> {
  const body = record(await paired("/remote/browser-credential", { method: "POST", body: JSON.stringify({ name }), headers: json }));
  const accessKey = text(body.accessKey);
  const discoverySecret = text(body.discoverySecret);
  if (!accessKey || !discoverySecret) throw new Error("The node sent no credential.");
  return { accessKey, discoverySecret, onion: text(body.onion) };
}

// This app's own link to a home node.

function parseLink(value: unknown): HomeLink {
  const body = record(value);
  if (body.paired !== true) return { paired: false };
  return { paired: true, onion: text(body.onion) ?? "", deviceId: text(body.deviceId) ?? "", name: text(body.name) ?? "", pairedAt: count(body.pairedAt), via: text(body.via) };
}

export async function fetchHomeLink(): Promise<HomeLink> {
  return parseLink(await ownNodeJson("/home"));
}

/** Send a pairing request to the home node; the answer carries the emoji to compare. */
export async function startHomePairing(code: string, name: string): Promise<HomePairing> {
  const body = record(await ownNodeJson("/home/pair", { method: "POST", body: JSON.stringify({ code: code.trim(), name: name.trim() }), headers: json }));
  const requestId = text(body.requestId);
  if (!requestId) throw new Error("The home node sent no request id.");
  return { requestId, sas: texts(body.sas), sasNames: texts(body.sasNames) };
}

/** Wait (up to about 25 s) for the owner's answer on the home node. */
export async function waitHomePairing(requestId: string): Promise<{ state: "pending" | "rejected" | "expired" } | { state: "approved"; link: HomeLink }> {
  const body = record(await ownNodeJson(`/home/pair/${encodeURIComponent(requestId)}`));
  if (body.state === "approved") return { state: "approved", link: parseLink({ ...body, paired: true }) };
  return { state: body.state === "rejected" || body.state === "expired" ? body.state : "pending" };
}

export async function forgetHome(): Promise<HomeLink> {
  return parseLink(await ownNodeJson("/home", { method: "DELETE" }));
}

// "Run this computer as a home node": a setting of the desktop shell, which
// restarts its own node with remote access.

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

function shellInvoke(): Invoke | null {
  const internals = (globalThis as { __TAURI_INTERNALS__?: { invoke?: Invoke } }).__TAURI_INTERNALS__;
  return typeof internals?.invoke === "function" ? internals.invoke : null;
}

export function canRunAsHomeNode(): boolean {
  return shellInvoke() !== null;
}

export async function homeNodeMode(): Promise<boolean> {
  return (await shellInvoke()?.("home_node_mode")) === true;
}

export async function setHomeNodeMode(on: boolean): Promise<boolean> {
  const invoke = shellInvoke();
  if (!invoke) throw new Error("Only the desktop app can run as a home node.");
  return (await invoke("set_home_node_mode", { on })) === true;
}

/** "5gyx6c…ttfad.onion" */
export function shortAddress(address: string): string {
  const [name, ...rest] = address.split(".");
  return name.length > 14 ? `${name.slice(0, 6)}…${name.slice(-5)}${rest.length ? "." + rest.join(".") : ""}` : address;
}
