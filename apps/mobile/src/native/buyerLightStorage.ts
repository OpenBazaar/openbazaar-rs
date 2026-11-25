import * as SecureStore from "expo-secure-store";

import type { BuyerLightStoreAdapter, LightOrderRecord } from "../buyerLight/contracts.ts";
import type { LightFetcher } from "../buyerLight/client.ts";
import { decodeResponse, encodeRequest } from "../homeNode/http1.ts";
import { channels } from "./channels.ts";
import { readSealed, writeSealed } from "./storage.ts";

const PHRASE_KEY = "ob.buyerLight.phrase";
const ORDERS_FILE = "buyer-light-orders.obv";
const LOCKED = { keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY };

/** Native encrypted store for buyer-only light mode (OS keystore + XChaCha20-Poly1305 sealed file). */
export const nativeBuyerLightStore: BuyerLightStoreAdapter = {
  async loadPhrase(): Promise<string | null> {
    return SecureStore.getItemAsync(PHRASE_KEY, LOCKED).catch(() => null);
  },
  async savePhrase(phrase: string): Promise<void> {
    await SecureStore.setItemAsync(PHRASE_KEY, phrase, LOCKED);
  },
  async loadOrders(): Promise<LightOrderRecord[]> {
    const raw = await readSealed(ORDERS_FILE);
    return Array.isArray(raw) ? (raw as LightOrderRecord[]) : [];
  },
  async saveOrders(orders: LightOrderRecord[]): Promise<void> {
    await writeSealed(ORDERS_FILE, orders);
  },
  async clearAll(): Promise<void> {
    await SecureStore.deleteItemAsync(PHRASE_KEY, LOCKED).catch(() => null);
    await writeSealed(ORDERS_FILE, []);
  },
};

/**
 * Native fetcher for buyer-only light mode: routes `.onion` stores through the
 * app's embedded Arti Tor client (`channels.tor`) and `https://` stores through
 * direct `fetch` without cookies or redirects.
 */
export const nativeLightFetcher: LightFetcher = async (url, init) => {
  const parsed = new URL(url);
  if (parsed.hostname.endsWith(".onion")) {
    const method = (init?.method ?? "GET").toUpperCase();
    const headers: Record<string, string> = {};
    if (init?.headers && typeof init.headers === "object" && !Array.isArray(init.headers)) {
      for (const [k, v] of Object.entries(init.headers as Record<string, string>)) {
        headers[k] = v;
      }
    }
    const body =
      init?.body instanceof Uint8Array
        ? init.body
        : typeof init?.body === "string"
          ? new TextEncoder().encode(init.body)
          : new Uint8Array(0);
    const path = `${parsed.pathname}${parsed.search}`;
    const rawRequest = encodeRequest(method, path, parsed.hostname, headers, body);
    const rawAnswer = await channels.tor(parsed.hostname, rawRequest, 60_000);
    const decoded = decodeResponse(rawAnswer);
    return new Response(new Uint8Array(decoded.body), {
      status: decoded.status,
      headers: decoded.headers,
    });
  }
  return globalThis.fetch(url, init);
};
