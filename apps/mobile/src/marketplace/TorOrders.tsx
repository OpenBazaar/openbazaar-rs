// Tor Browser / guest mode "My Orders" view + "Restore with 12 words" flow.
//
// Reads locally persisted pull-mode orders from `createWebBuyerStore()`, lets
// the buyer reopen any order in the shared `TorOrderDetail` view, refresh
// statuses across sellers, view/copy their 12-word BIP-39 receipt code, or
// restore orders in a fresh Tor Browser session from their 12 words + store URL.

import { useEffect, useMemo, useState } from "react";
import type { LightOrderRecord } from "../buyerLight/contracts.ts";
import {
  pullLightOrderStatus,
  recoverLightOrdersFromStore,
} from "../buyerLight/client.ts";
import {
  buyerSeedFromPhrase,
  DEFAULT_BUYER_NETWORK,
  isValidBuyerPhrase,
  normalizeBuyerPhrase,
  type BuyerSeed,
} from "../buyerLight/keys.ts";
import { createWebBuyerStore } from "../buyerLight/webStore.ts";
import { CopyButton } from "./CopyAddress.tsx";
import { PageHeading } from "./PageHeading.tsx";
import { TorOrderDetail, torStatusMeta, WordGrid } from "./TorOrderDetail.tsx";

const store = createWebBuyerStore();

export type KnownVendorStore = {
  readonly name: string;
  readonly url: string;
};

function formatOrderDate(unixSecs: number): string {
  try {
    return new Date(unixSecs * 1000).toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  } catch {
    return "";
  }
}

function mergeOrders(
  existing: readonly LightOrderRecord[],
  incoming: readonly LightOrderRecord[],
): LightOrderRecord[] {
  const byId = new Map<string, LightOrderRecord>();
  for (const order of existing) byId.set(order.id, order);
  for (const order of incoming) byId.set(order.id, order);
  return [...byId.values()].sort((a, b) => b.createdAt - a.createdAt);
}

export function TorOrders({
  knownStores,
  onBrowse,
}: {
  knownStores: readonly KnownVendorStore[];
  onBrowse: () => void;
}) {
  const [booting, setBooting] = useState(true);
  const [phrase, setPhrase] = useState<string | null>(null);
  const [orders, setOrders] = useState<LightOrderRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showReceiptCode, setShowReceiptCode] = useState(false);
  const [showRestore, setShowRestore] = useState(false);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    void Promise.all([store.loadPhrase(), store.loadOrders()]).then(([savedPhrase, savedOrders]) => {
      if (!active) return;
      setPhrase(savedPhrase && isValidBuyerPhrase(savedPhrase) ? savedPhrase : null);
      setOrders([...savedOrders].sort((a, b) => b.createdAt - a.createdAt));
      setBooting(false);
    });
    return () => { active = false; };
  }, []);

  const seed = useMemo<BuyerSeed | null>(() => {
    if (!phrase) return null;
    try {
      return buyerSeedFromPhrase(phrase, DEFAULT_BUYER_NETWORK);
    } catch {
      return null;
    }
  }, [phrase]);

  const persistOrders = (next: LightOrderRecord[]) => {
    const sorted = [...next].sort((a, b) => b.createdAt - a.createdAt);
    setOrders(sorted);
    void store.saveOrders(sorted);
  };

  const upsertOrder = (updated: LightOrderRecord) => {
    persistOrders([updated, ...orders.filter((o) => o.id !== updated.id)]);
  };

  const refreshAll = async () => {
    if (!seed || refreshingAll || orders.length === 0) return;
    setRefreshingAll(true);
    setError("");
    setNotice("");
    let updatedCount = 0;
    const failed: string[] = [];
    let latest = [...orders];
    for (const order of orders) {
      try {
        const refreshed = await pullLightOrderStatus(order, seed);
        latest = [refreshed, ...latest.filter((o) => o.id !== refreshed.id)];
        updatedCount += 1;
      } catch (err) {
        failed.push(`${order.title}: ${err instanceof Error ? err.message : "could not refresh"}`);
      }
    }
    persistOrders(latest);
    setRefreshingAll(false);
    if (updatedCount > 0) {
      setNotice(`Refreshed ${updatedCount} ${updatedCount === 1 ? "order" : "orders"} from ${updatedCount === 1 ? "the seller" : "sellers"}.`);
    }
    if (failed.length > 0) {
      setError(failed[0]);
    }
  };

  if (booting) {
    return (
      <section className="ob-tor-checkout">
        <p className="ob-meta" role="status">Loading your orders…</p>
      </section>
    );
  }

  const selectedOrder = selectedId ? orders.find((o) => o.id === selectedId) ?? null : null;
  if (selectedOrder && seed) {
    return (
      <TorOrderDetail
        order={selectedOrder}
        seed={seed}
        onUpdated={upsertOrder}
        onBack={() => setSelectedId(null)}
        backLabel="Back to my orders"
      />
    );
  }

  return (
    <section className="ob-tor-checkout" aria-labelledby="ob-tor-orders-title">
      <div className="ob-tor-orders-head">
        <PageHeading
          title="My orders"
          description="Your direct-pay orders saved in this browser. Refresh status or restore from your 12-word receipt code anytime."
        />
        <div className="ob-tor-status-actions">
          {orders.length > 0 && seed && (
            <button
              className="ob-secondary"
              type="button"
              disabled={refreshingAll}
              onClick={() => void refreshAll()}
            >
              {refreshingAll ? "Refreshing…" : "Refresh all"}
            </button>
          )}
          {orders.length > 0 && (
            <button
              className="ob-secondary"
              type="button"
              onClick={() => {
                setShowRestore((v) => !v);
                setError("");
                setNotice("");
              }}
            >
              {showRestore ? "Hide restore" : "Restore with 12 words"}
            </button>
          )}
        </div>
      </div>

      {notice && <p className="ob-tor-notice" role="status">{notice}</p>}
      {error && <p className="ob-error" role="alert">{error}</p>}

      {/* Expandable 12-word receipt code card */}
      {phrase && (
        <div className="ob-tor-card">
          <div className="ob-tor-card-header">
            <h3>Your 12-word receipt code</h3>
            <div className="ob-tor-card-actions">
              <CopyButton value={phrase} label="Copy 12 words" />
              <button
                className="ob-text-link"
                type="button"
                aria-expanded={showReceiptCode}
                onClick={() => setShowReceiptCode((v) => !v)}
              >
                {showReceiptCode ? "Hide" : "Show"}
              </button>
            </div>
          </div>
          <p className="ob-meta">
            Keep these 12 words offline to recover your orders and check status in a new Tor Browser session.
          </p>
          {showReceiptCode && <WordGrid words={phrase.split(" ")} />}
        </div>
      )}

      {/* Restore with 12 words panel (always shown in empty state, toggleable otherwise) */}
      {(orders.length === 0 || showRestore) && (
        <RestoreOrdersCard
          knownStores={knownStores}
          initialPhrase={phrase ?? ""}
          onRestored={(cleanPhrase, recovered) => {
            void store.savePhrase(cleanPhrase);
            setPhrase(cleanPhrase);
            const merged = mergeOrders(orders, recovered);
            persistOrders(merged);
            setShowRestore(false);
            setError("");
            setNotice(
              recovered.length > 0
                ? `Restored ${recovered.length} ${recovered.length === 1 ? "order" : "orders"} from your 12-word receipt code.`
                : "Receipt code saved. No matching orders were found at the selected store.",
            );
          }}
        />
      )}

      {/* Orders list */}
      {orders.length > 0 ? (
        <div className="ob-tor-orders-list">
          {orders.map((order) => {
            const { badgeClass, label: statusLabel } = torStatusMeta(order);
            return (
              <button
                key={order.id}
                type="button"
                className="ob-tor-order-row"
                onClick={() => setSelectedId(order.id)}
              >
                <div className="ob-tor-order-row-top">
                  <strong className="ob-tor-order-title">{order.title}</strong>
                  <span className={`ob-tor-status-badge ${badgeClass}`}>{statusLabel}</span>
                </div>
                <div className="ob-tor-order-row-meta">
                  <span>{order.sellerName}</span>
                  <span>
                    {order.quantity} × · {order.totalSats.toLocaleString()} sats
                  </span>
                </div>
                <div className="ob-tor-order-row-foot">
                  <code>#{order.id.slice(0, 8)}</code>
                  <span>{formatOrderDate(order.createdAt)}</span>
                </div>
              </button>
            );
          })}
        </div>
      ) : (
        <div className="ob-tor-card">
          <h3>No orders saved in this browser yet</h3>
          <p className="ob-meta">
            Place a direct-pay order on any product, or restore previous orders above with your 12-word receipt code.
          </p>
          <div className="ob-tor-status-actions">
            <button className="ob-primary" type="button" onClick={onBrowse}>
              Browse products
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

/* ── Restore with 12 words card ───────────────────────────────── */

function RestoreOrdersCard({
  knownStores,
  initialPhrase,
  onRestored,
}: {
  knownStores: readonly KnownVendorStore[];
  initialPhrase: string;
  onRestored: (cleanPhrase: string, recovered: LightOrderRecord[]) => void;
}) {
  const defaultTarget = knownStores.length > 1 ? "__all__" : (knownStores[0]?.url ?? "__custom__");
  const [phraseText, setPhraseText] = useState(initialPhrase);
  const [target, setTarget] = useState(defaultTarget);
  const [customUrl, setCustomUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (target === "__custom__" && knownStores.length > 0 && !customUrl) {
      setTarget(knownStores.length > 1 ? "__all__" : knownStores[0].url);
    }
  }, [knownStores, target, customUrl]);

  const handleRestore = async () => {
    if (busy) return;
    if (!isValidBuyerPhrase(phraseText)) {
      setError("Enter a valid 12-word BIP-39 receipt code.");
      return;
    }
    const cleanPhrase = normalizeBuyerPhrase(phraseText);
    const urls: string[] =
      target === "__all__"
        ? knownStores.map((s) => s.url)
        : target === "__custom__"
          ? [customUrl.trim()].filter(Boolean)
          : [target];

    if (urls.length === 0) {
      setError("Choose a store or enter a seller's .onion URL.");
      return;
    }

    setBusy(true);
    setError("");
    try {
      const seed = buyerSeedFromPhrase(cleanPhrase, DEFAULT_BUYER_NETWORK);
      const allRecovered: LightOrderRecord[] = [];
      let lastErr: string | null = null;
      for (const url of urls) {
        try {
          const found = await recoverLightOrdersFromStore({ seed, vendorUrl: url });
          allRecovered.push(...found);
        } catch (err) {
          lastErr = err instanceof Error ? err.message : `Could not reach ${url}.`;
        }
      }
      if (allRecovered.length === 0 && lastErr && urls.length === 1) {
        setError(lastErr);
        return;
      }
      onRestored(cleanPhrase, allRecovered);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not restore orders.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="ob-tor-card">
      <h3>Restore with 12 words</h3>
      <p className="ob-meta">
        Enter your 12-word receipt code to pull your order status, payment details, and seller messages directly from the seller's node.
      </p>
      {error && <p className="ob-error" role="alert">{error}</p>}
      <label className="ob-tor-compact-field">
        <span>12-word receipt code</span>
        <textarea
          className="ob-textarea"
          value={phraseText}
          onChange={(e) => setPhraseText(e.target.value)}
          placeholder="twelve space-separated BIP 39 words"
          rows={3}
          spellCheck={false}
          autoCapitalize="off"
        />
      </label>

      {knownStores.length > 0 ? (
        <label className="ob-tor-compact-field">
          <span>Store to check</span>
          <select
            className="ob-tor-select"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
          >
            {knownStores.length > 1 && (
              <option value="__all__">All known stores ({knownStores.length})</option>
            )}
            {knownStores.map((s) => (
              <option key={s.url} value={s.url}>
                {s.name} ({s.url.replace(/^https?:\/\//, "").slice(0, 20)}…)
              </option>
            ))}
            <option value="__custom__">Custom store .onion URL…</option>
          </select>
        </label>
      ) : null}

      {(target === "__custom__" || knownStores.length === 0) && (
        <label className="ob-tor-compact-field">
          <span>Seller store URL</span>
          <input
            className="ob-tor-input"
            type="text"
            value={customUrl}
            onChange={(e) => setCustomUrl(e.target.value)}
            placeholder="http://….onion"
            spellCheck={false}
            autoCapitalize="off"
          />
        </label>
      )}

      <div className="ob-tor-status-actions">
        <button
          className="ob-primary"
          type="button"
          disabled={busy || !phraseText.trim()}
          onClick={() => void handleRestore()}
        >
          {busy ? "Restoring orders…" : "Restore orders"}
        </button>
      </div>
    </div>
  );
}
