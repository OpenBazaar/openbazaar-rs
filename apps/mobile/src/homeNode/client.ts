import { bytesToHex, hexToBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { decodeResponse, encodeRequest, type RawResponse } from "./http1.ts";
import { lanSeal } from "./lan.ts";
import { requestPreimage } from "./protocol.ts";

// Signed requests to a paired home node (protocol v2, "Signed requests" and
// "Transports"). The LAN channel is tried first when the node may be on this
// network; Tor is the fallback and the default.

export type Signer = { deviceId: string; sign(preimage: string): Promise<string> };

export type NodeLink = {
  nodeId: string;
  nodeKey: string;
  onion: string;
  lan: string[];
  deviceId: string;
};

export type Channels = {
  /** Raw HTTP/1.1 bytes to `{onion}.onion:80`; the raw answer. */
  tor(onion: string, request: Uint8Array, timeoutMs: number): Promise<Uint8Array>;
  /** `POST http://{address}/lan`; status and body. */
  lan(address: string, body: Uint8Array, timeoutMs: number): Promise<{ status: number; body: Uint8Array }>;
};

export type Via = "lan" | "tor";
export type ClientState = { via: Via | null; inFlight: number };

export const LAN_PORT = 8790;
export const LAN_PROBE_MS = 1500;
const LAN_FRESH_MS = 60_000;
export const TOR_TIMEOUT_MS = 90_000;
/** The Host header of requests inside the LAN channel. */
export const LAN_HOST = "lan";
const LAN_TIMEOUT_MS = 30_000;
export const MAX_ANSWER = 16 * 1024 * 1024;

export class UnknownDeviceError extends Error {
  constructor() {
    super("Your home node no longer knows this phone. Pair it again.");
    this.name = "UnknownDeviceError";
  }
}

/** A mutation sent on the LAN channel whose answer was lost; it may have run. */
export class AnswerLostError extends Error {
  constructor() {
    super("The connection to your node dropped before it answered. Check whether the change was made before trying again.");
    this.name = "AnswerLostError";
  }
}

export function lanAddresses(link: Pick<NodeLink, "lan">, discovered: string[] = []): string[] {
  return [...new Set([...discovered, ...link.lan, `openbazaar.local:${LAN_PORT}`])];
}

export type RequestOptions = { signer?: Signer; idempotent?: boolean; timeoutMs?: number; contentType?: string };

export class HomeNodeClient {
  private clockOffset = 0;
  private lanAt: { address: string; at: number } | null = null;
  private lanDownAt = 0;
  private state: ClientState = { via: null, inFlight: 0 };
  private listeners = new Set<(state: ClientState) => void>();
  discovered: string[] = [];
  link: NodeLink;
  private readonly signer: Signer;
  private readonly channels: Channels;
  private readonly onForget: () => void;
  private readonly clock: () => number;

  constructor(link: NodeLink, signer: Signer, channels: Channels, onForget: () => void, clock: () => number = () => Date.now()) {
    this.link = link;
    this.signer = signer;
    this.channels = channels;
    this.onForget = onForget;
    this.clock = clock;
  }

  subscribe(listener: (state: ClientState) => void): () => void {
    this.listeners.add(listener);
    listener(this.state);
    return () => this.listeners.delete(listener);
  }

  private publish(next: Partial<ClientState>) {
    this.state = { ...this.state, ...next };
    for (const listener of this.listeners) listener(this.state);
  }

  now(): number {
    return Math.floor(this.clock() / 1000) + this.clockOffset;
  }

  async request(method: string, path: string, body: Uint8Array = new Uint8Array(), options: RequestOptions = {}): Promise<RawResponse & { via: Via }> {
    this.publish({ inFlight: this.state.inFlight + 1 });
    try {
      for (let attempt = 0; ; attempt++) {
        const answer = await this.send(method, path, body, options);
        if (answer.status === 401) {
          const detail = jsonOf(answer.body);
          if (detail.code === "unknown_device") {
            this.onForget();
            throw new UnknownDeviceError();
          }
          if (detail.code === "clock" && typeof detail.serverTime === "number" && attempt === 0) {
            this.clockOffset = detail.serverTime - Math.floor(this.clock() / 1000);
            continue;
          }
        }
        this.publish({ via: answer.via });
        return answer;
      }
    } finally {
      this.publish({ inFlight: this.state.inFlight - 1 });
    }
  }

  private async signedBytes(method: string, path: string, body: Uint8Array, signer: Signer, host: string, contentType?: string): Promise<Uint8Array> {
    const time = this.now();
    const nonce = bytesToHex(randomBytes(16));
    const signature = await signer.sign(requestPreimage(this.link.nodeId, signer.deviceId, time, nonce, method, path, body));
    const headers: Record<string, string> = {
      "X-Ob-Device": signer.deviceId,
      "X-Ob-Time": String(time),
      "X-Ob-Nonce": nonce,
      "X-Ob-Signature": signature,
    };
    if (body.length) headers["Content-Type"] = contentType ?? "application/json";
    return encodeRequest(method, path, host, headers, body);
  }

  private async send(method: string, path: string, body: Uint8Array, options: RequestOptions): Promise<RawResponse & { via: Via }> {
    const signer = options.signer ?? this.signer;
    const address = await this.lanAddress();
    if (address) {
      const inner = await this.signedBytes(method, path, body, signer, LAN_HOST, options.contentType);
      try {
        return { ...(await lanExchange(this.channels, address, hexToBytes(this.link.nodeKey), inner, options.timeoutMs ?? LAN_TIMEOUT_MS)), via: "lan" };
      } catch {
        this.lanAt = null;
        this.lanDownAt = this.clock();
        if (method !== "GET" && !options.idempotent) throw new AnswerLostError();
      }
    }
    const request = await this.signedBytes(method, path, body, signer, `${this.link.onion}.onion`, options.contentType);
    const raw = await this.channels.tor(this.link.onion, request, options.timeoutMs ?? TOR_TIMEOUT_MS);
    return { ...decodeResponse(raw), via: "tor" };
  }

  /** A LAN address that answered recently, or the first to answer a probe within 1.5 s. */
  private async lanAddress(): Promise<string | null> {
    const now = this.clock();
    if (this.lanAt && now - this.lanAt.at < LAN_FRESH_MS) return this.lanAt.address;
    if (now - this.lanDownAt < LAN_FRESH_MS) return null;
    const address = await probeLan(this.channels, lanAddresses(this.link, this.discovered), hexToBytes(this.link.nodeKey));
    if (address) this.lanAt = { address, at: now };
    else this.lanDownAt = now;
    return address;
  }
}

/** One sealed request on the LAN channel; throws if the node did not answer under its key. */
export async function lanExchange(channels: Channels, address: string, nodePublic: Uint8Array, inner: Uint8Array, timeoutMs: number): Promise<RawResponse> {
  const sealed = lanSeal(nodePublic, inner);
  const answer = await channels.lan(address, sealed.body, timeoutMs);
  if (answer.status !== 200) throw new Error(`The LAN channel answered ${answer.status}.`);
  return decodeResponse(sealed.open(answer.body));
}

/** The first address whose node opens a sealed probe, or null after 1.5 s. */
export async function probeLan(channels: Channels, addresses: string[], nodePublic: Uint8Array): Promise<string | null> {
  if (!addresses.length) return null;
  const probe = encodeRequest("GET", "/remote/hello", LAN_HOST, {}, new Uint8Array());
  const attempts = addresses.map(address => lanExchange(channels, address, nodePublic, probe, LAN_PROBE_MS).then(() => address));
  const timeout = new Promise<null>(resolve => setTimeout(() => resolve(null), LAN_PROBE_MS));
  return Promise.race([firstFulfilled(attempts), timeout]);
}

function firstFulfilled<T>(promises: Promise<T>[]): Promise<T | null> {
  return new Promise(resolve => {
    let left = promises.length;
    for (const promise of promises) {
      promise.then(resolve, () => {
        if (--left === 0) resolve(null);
      });
    }
  });
}

export function jsonOf(body: Uint8Array): Record<string, unknown> {
  try {
    const value: unknown = JSON.parse(new TextDecoder().decode(body));
    return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

export function jsonBody(value: unknown): Uint8Array {
  return utf8ToBytes(JSON.stringify(value));
}
