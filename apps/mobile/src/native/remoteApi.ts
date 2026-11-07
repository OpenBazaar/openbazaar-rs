import { jsonBody, jsonOf } from "../homeNode/client.ts";
import { homeClient } from "./session.ts";

// The /remote/* routes a paired device uses (docs/home-node-protocol.md, "Devices and push").

export type Device = {
  id: string;
  name: string;
  platform: string;
  kind: "app" | "browser";
  pairedAt: number;
  lastSeen: number | null;
  lastVia: "tor" | "lan" | "local" | null;
  pairedBy: string | null;
  push: { enabled: boolean; platform: string | null };
  thisDevice: boolean;
};

export type NewPairingCode = { link: string | null; words: string[]; expiresInSecs: number };

async function call(method: string, path: string, body?: unknown): Promise<Record<string, unknown>> {
  const answer = await homeClient().request(method, path, body === undefined ? undefined : jsonBody(body));
  const value = jsonOf(answer.body);
  if (answer.status < 200 || answer.status >= 300) {
    throw new Error(typeof value.error === "string" ? value.error : `The node answered ${answer.status}.`);
  }
  return value;
}

const text = (value: unknown) => (typeof value === "string" ? value : null);
const num = (value: unknown) => (typeof value === "number" ? value : null);

function parseDevice(value: unknown): Device {
  const item = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  const push = (item.push && typeof item.push === "object" ? item.push : {}) as Record<string, unknown>;
  const via = item.lastVia;
  return {
    id: text(item.id) ?? "",
    name: text(item.name) ?? "Unnamed device",
    platform: text(item.platform) ?? "",
    kind: item.kind === "browser" ? "browser" : "app",
    pairedAt: num(item.pairedAt) ?? 0,
    lastSeen: num(item.lastSeen),
    lastVia: via === "tor" || via === "lan" || via === "local" ? via : null,
    pairedBy: text(item.pairedBy),
    push: { enabled: push.enabled === true, platform: text(push.platform) },
    thisDevice: item.thisDevice === true,
  };
}

export async function fetchDevices(): Promise<Device[]> {
  const body = await call("GET", "/remote/devices");
  return (Array.isArray(body.devices) ? body.devices : []).map(parseDevice);
}

export async function removeDevice(id: string, rotate: boolean): Promise<void> {
  await call("DELETE", `/remote/devices/${encodeURIComponent(id)}`, rotate ? { rotate: true } : undefined);
}

/** A code for another device. The node may answer with a link, words, or both. */
export async function createPairingCode(): Promise<NewPairingCode> {
  const body = await call("POST", "/remote/pairing", {});
  const words = Array.isArray(body.words) ? body.words.filter((w): w is string => typeof w === "string") : typeof body.words === "string" ? body.words.split(/\s+/) : [];
  return { link: text(body.link) ?? text(body.url) ?? text(body.code), words, expiresInSecs: num(body.expiresInSecs) ?? 600 };
}

export type PendingRequest = { requestId: string; name: string; platform: string; sas: string[]; sasNames: string[]; ageSecs: number };

export async function fetchPending(): Promise<PendingRequest[]> {
  const body = await call("GET", "/remote/pending");
  const strings = (value: unknown) => (Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : []);
  return (Array.isArray(body.pending) ? body.pending : []).map(value => {
    const item = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
    return { requestId: text(item.requestId) ?? "", name: text(item.name) ?? "", platform: text(item.platform) ?? "", sas: strings(item.sas), sasNames: strings(item.sasNames), ageSecs: num(item.ageSecs) ?? 0 };
  });
}

export async function decidePending(requestId: string, approve: boolean): Promise<void> {
  await call("POST", `/remote/pending/${encodeURIComponent(requestId)}`, { approve });
}

export async function enablePush(platform: "apns" | "fcm", token: string): Promise<void> {
  await call("PUT", "/remote/push", { platform, token });
}

export async function disablePush(): Promise<void> {
  await call("DELETE", "/remote/push");
}
