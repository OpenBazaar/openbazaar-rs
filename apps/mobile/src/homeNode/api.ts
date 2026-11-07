import { ownNodeJson, paired } from "../catalog.ts";

export type RemoteDevice = {
  id: string;
  name: string;
  pairedAt: number;
  lastSeen: number;
  pairedBy: string | null;
  thisDevice: boolean;
};

export type RemoteStatus = {
  enabled: boolean;
  onion: string | null;
  onionState: string | null;
  devices: number;
  webShop: boolean;
};

export type HomeLink = { paired: false } | { paired: true; address: string; deviceId: string; name: string; pairedAt: number };

const json = { "Content-Type": "application/json" };
const record = (value: unknown): Record<string, unknown> => (value && typeof value === "object" ? (value as Record<string, unknown>) : {});
const text = (value: unknown): string | null => (typeof value === "string" ? value : null);
const count = (value: unknown): number => (typeof value === "number" && Number.isFinite(value) ? value : 0);

function parseDevices(value: unknown): RemoteDevice[] {
  const list = record(value).devices;
  return (Array.isArray(list) ? list : []).map(record).map(item => ({
    id: text(item.id) ?? "",
    name: text(item.name) ?? "Unnamed device",
    pairedAt: count(item.pairedAt),
    lastSeen: count(item.lastSeen),
    pairedBy: text(item.pairedBy),
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
    devices: count(body.devices),
    webShop: body.webShop === true,
  };
}

export async function fetchDevices(): Promise<RemoteDevice[]> {
  return parseDevices(await paired("/remote/devices"));
}

export async function revokeDevice(id: string): Promise<RemoteDevice[]> {
  return parseDevices(await paired(`/remote/devices/${encodeURIComponent(id)}`, { method: "DELETE" }));
}

export async function createPairingCode(): Promise<{ code: string; expiresInSecs: number }> {
  const body = record(await paired("/remote/pairing", { method: "POST", body: "{}", headers: json }));
  const code = text(body.code);
  if (!code) throw new Error("The node sent no pairing code.");
  return { code, expiresInSecs: count(body.expiresInSecs) };
}

// This app's own link to a home node.

function parseLink(value: unknown): HomeLink {
  const body = record(value);
  if (body.paired !== true) return { paired: false };
  return { paired: true, address: text(body.address) ?? "", deviceId: text(body.deviceId) ?? "", name: text(body.name) ?? "", pairedAt: count(body.pairedAt) };
}

export async function fetchHomeLink(): Promise<HomeLink> {
  return parseLink(await ownNodeJson("/home"));
}

export async function pairHome(code: string, name: string): Promise<HomeLink> {
  return parseLink(await ownNodeJson("/home/pair", { method: "POST", body: JSON.stringify({ code: code.trim(), name: name.trim() }), headers: json }));
}

export async function forgetHome(): Promise<HomeLink> {
  return parseLink(await ownNodeJson("/home", { method: "DELETE" }));
}

/** "5gyx6c…ttfad.onion" */
export function shortAddress(address: string): string {
  const [name, ...rest] = address.split(".");
  return name.length > 14 ? `${name.slice(0, 6)}…${name.slice(-5)}${rest.length ? "." + rest.join(".") : ""}` : address;
}
