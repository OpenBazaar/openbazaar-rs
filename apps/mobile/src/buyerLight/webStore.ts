// localStorage-backed BuyerLightStoreAdapter for Tor Browser direct-pay checkout.
//
// Keys are prefixed so they don't collide with the shop page's own storage.
// All reads/writes are synchronous under the hood but wrapped in Promises to
// match the BuyerLightStoreAdapter interface used by signed native code.

import type { BuyerLightStoreAdapter, LightOrderRecord } from "./contracts.ts";

const PREFIX = "ob-buyer-light-";
const PHRASE_KEY = `${PREFIX}phrase`;
const ORDERS_KEY = `${PREFIX}orders`;

export function createWebBuyerStore(): BuyerLightStoreAdapter {
  return {
    async loadPhrase() {
      try {
        return localStorage.getItem(PHRASE_KEY);
      } catch {
        return null;
      }
    },
    async savePhrase(phrase: string) {
      try {
        localStorage.setItem(PHRASE_KEY, phrase);
      } catch {
        // Storage unavailable — the phrase lives only in memory this session.
      }
    },
    async loadOrders() {
      try {
        const raw = localStorage.getItem(ORDERS_KEY);
        if (!raw) return [];
        return JSON.parse(raw) as LightOrderRecord[];
      } catch {
        return [];
      }
    },
    async saveOrders(orders: LightOrderRecord[]) {
      try {
        localStorage.setItem(ORDERS_KEY, JSON.stringify(orders));
      } catch {
        // Storage full or unavailable.
      }
    },
    async clearAll() {
      try {
        localStorage.removeItem(PHRASE_KEY);
        localStorage.removeItem(ORDERS_KEY);
      } catch {
        // No-op.
      }
    },
  };
}
