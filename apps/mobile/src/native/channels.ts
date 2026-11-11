import { fetch as expoFetch } from "expo/fetch";
import { base64 } from "@scure/base";

import { deviceLog, insertDiscoveryKey, startTor, torExchange, torStatus, type TorStatus } from "../../modules/ob-tor";
import { MAX_ANSWER, type Channels } from "../homeNode/client.ts";
import { logLine } from "./logLine.ts";

/** A line in the device log. Never pass a pairing code, link, token or key. */
export const log = (text: string): void => deviceLog(logLine(text));

// The two ways to reach the node: embedded Arti, and the LAN channel over
// plain HTTP (sealed end to end by lan.ts, so the transport adds nothing).

let starting: Promise<void> | null = null;

export function ensureTor(): Promise<void> {
  starting ??= startTor().catch(err => {
    starting = null;
    log(`tor: could not start: ${err instanceof Error ? err.message : err}`);
    throw err;
  });
  return starting;
}

export async function waitForTor(timeoutMs = 120_000, onStatus?: (status: TorStatus) => void): Promise<void> {
  await ensureTor();
  const until = Date.now() + timeoutMs;
  for (;;) {
    const status = torStatus();
    onStatus?.(status);
    if (status.state === "ready") return;
    if (Date.now() > until) {
      log(`tor: not connected after ${Math.round(timeoutMs / 1000)}s (${status.state}, ${Math.round(status.progress * 100)}%: ${status.summary})`);
      throw new Error("Tor is taking too long to connect. Check this phone's internet connection.");
    }
    await new Promise(resolve => setTimeout(resolve, 500));
  }
}

export { insertDiscoveryKey, torStatus };

export const channels: Channels = {
  async tor(onion, request, timeoutMs) {
    await ensureTor();
    return base64.decode(await torExchange(onion, 80, base64.encode(request), MAX_ANSWER, timeoutMs));
  },
  async lan(address, body, timeoutMs) {
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), timeoutMs);
    try {
      const response = await expoFetch(`http://${address}/lan`, {
        method: "POST",
        headers: { "Content-Type": "application/octet-stream" },
        body: new Uint8Array(body),
        signal: abort.signal,
      });
      return { status: response.status, body: new Uint8Array(await response.arrayBuffer()) };
    } finally {
      clearTimeout(timer);
    }
  },
};
