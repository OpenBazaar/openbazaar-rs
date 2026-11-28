// Shared Tor Browser order detail view & UI primitives used by both
// TorCheckout (immediately after placing an order) and TorOrders (when
// reopening a saved or restored order).

import { useEffect, useState } from "react";
import QRCode from "qrcode";
import type { LightOrderRecord } from "../buyerLight/contracts.ts";
import {
  confirmLightOrderComplete,
  pullLightOrderStatus,
  sendLightOrderMessage,
} from "../buyerLight/client.ts";
import type { BuyerSeed } from "../buyerLight/keys.ts";
import { CopyButton } from "./CopyAddress.tsx";
import { PageHeading } from "./PageHeading.tsx";

/* ── Shared: status pill metadata ─────────────────────────────── */

export function torStatusMeta(order: LightOrderRecord): { badgeClass: string; label: string } {
  const badgeClass =
    order.status === "Awaiting payment" || order.status === "Awaiting funding" ? "is-waiting" :
    order.status === "Payment seen" ? "is-seen" :
    order.status === "Paid" || order.status === "Funded" ? "is-paid" :
    order.status === "Shipped" ? "is-shipped" :
    order.status === "Completed" ? "is-complete" :
    order.status === "Declined by the seller" || order.status === "Disputed" ? "is-declined" : "";

  const label =
    order.status === "Payment seen"
      ? order.confirmations === 0
        ? "Payment seen (unconfirmed)"
        : "Payment seen (confirming)"
      : order.status;

  return { badgeClass, label };
}

/* ── Shared: QR code rendered locally (works over Tor) ────────── */

export function TorPaymentQr({ value, label = "Bitcoin payment QR code" }: { value: string; label?: string }) {
  const [src, setSrc] = useState("");
  const [error, setError] = useState(false);
  useEffect(() => {
    let active = true;
    setSrc("");
    setError(false);
    void QRCode.toString(value, {
      type: "svg",
      margin: 2,
      errorCorrectionLevel: "M",
      color: { dark: "#000000", light: "#ffffff" },
    }).then(
      (svg: string) => {
        if (active) setSrc(`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`);
      },
      () => { if (active) setError(true); },
    );
    return () => { active = false; };
  }, [value]);

  if (error) return <p className="ob-tor-qr-error" role="alert">QR code unavailable. Copy the address instead.</p>;
  if (!src) return <p className="ob-meta" role="status">Preparing QR code…</p>;
  return (
    <div className="ob-tor-qr-frame">
      <img className="ob-tor-qr-img" width={200} height={200} src={src} alt={label} data-payment-uri={value} />
    </div>
  );
}

/* ── Shared: 3×4 word grid with numbered chips ────────────────── */

export function WordGrid({ words, copyValue }: { words: string[]; copyValue?: string }) {
  return (
    <div className="ob-tor-word-card">
      <div className="ob-tor-word-grid">
        {words.map((w, i) => (
          <span key={i} className="ob-tor-word-chip">
            <span className="ob-tor-word-num">{i + 1}</span>
            <span>{w}</span>
          </span>
        ))}
      </div>
      {copyValue && (
        <div className="ob-tor-word-copy">
          <CopyButton value={copyValue} label="Copy all 12 words" />
        </div>
      )}
    </div>
  );
}

/* ── Shared: monospace code box with inline copy ──────────────── */

export function CodeCopyRow({ value, label }: { value: string; label: string }) {
  return (
    <div className="ob-tor-code-row">
      <code>{value}</code>
      <CopyButton value={value} label={label} />
    </div>
  );
}

/* ── Shared: Order detail & live seller status / chat ─────────── */

export function TorOrderDetail({
  order,
  seed,
  onUpdated,
  onBack,
  backLabel,
}: {
  order: LightOrderRecord;
  seed: BuyerSeed;
  onUpdated: (order: LightOrderRecord) => void;
  onBack: () => void;
  backLabel: string;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [chatDraft, setChatDraft] = useState("");

  const pullStatus = async () => {
    setBusy(true);
    setError("");
    try {
      const updated = await pullLightOrderStatus(order, seed);
      onUpdated(updated);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not refresh status.");
    } finally {
      setBusy(false);
    }
  };

  const sendMessage = async () => {
    if (!chatDraft.trim() || busy) return;
    setBusy(true);
    setError("");
    try {
      const updated = await sendLightOrderMessage(order, seed, chatDraft);
      setChatDraft("");
      onUpdated(updated);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not send message.");
    } finally {
      setBusy(false);
    }
  };

  const confirmArrival = async () => {
    setBusy(true);
    setError("");
    try {
      const updated = await confirmLightOrderComplete(order, seed);
      onUpdated(updated);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not confirm arrival.");
    } finally {
      setBusy(false);
    }
  };

  const { badgeClass, label: statusLabel } = torStatusMeta(order);

  return (
    <section className="ob-tor-checkout">
      <button className="ob-back" type="button" onClick={onBack}>
        {backLabel}
      </button>
      <PageHeading
        title={`Order: ${order.title}`}
        description={`${order.sellerName} · #${order.id.slice(0, 8)}`}
      />
      {error && <p className="ob-error" role="alert">{error}</p>}
      {order.declinedReason && <p className="ob-error" role="alert">{order.declinedReason}</p>}

      {/* Status card */}
      <div className="ob-tor-card">
        <div className="ob-tor-status-header">
          <span className={`ob-tor-status-badge ${badgeClass}`}>{statusLabel}</span>
          <strong className="ob-tor-total">{order.totalSats.toLocaleString()} sats</strong>
        </div>
        {order.status === "Payment seen" && (
          <p className="ob-meta">
            {order.confirmations === 0
              ? "Detected in the mempool — waiting for 1 block confirmation."
              : `Confirming on-chain (${order.confirmations} confirmation${order.confirmations === 1 ? "" : "s"}) — waiting for finality.`}
          </p>
        )}
        {order.receivedSats > 0 && (
          <p className="ob-meta">
            Received {order.receivedSats.toLocaleString()} sats · {order.confirmations} confirmation{order.confirmations === 1 ? "" : "s"}
          </p>
        )}
        <div className="ob-tor-status-actions">
          <button className="ob-secondary" type="button" disabled={busy} onClick={() => void pullStatus()}>
            {busy ? "Refreshing…" : "Refresh status from seller"}
          </button>
          {order.status === "Shipped" && (
            <button className="ob-primary" type="button" disabled={busy} onClick={() => void confirmArrival()}>
              Confirm it arrived
            </button>
          )}
        </div>
      </div>

      {/* Payment card */}
      {order.paymentAddress && order.paymentUri && (
        <div className="ob-tor-card">
          <h3>Pay from any Bitcoin wallet</h3>
          <p className="ob-meta">
            Send exactly <strong>{order.totalSats.toLocaleString()} sats</strong> to the address below. Scan the QR code or copy the address.
          </p>
          <TorPaymentQr value={order.paymentUri} />
          <CodeCopyRow value={order.paymentAddress} label="Copy address" />
          <CodeCopyRow value={order.paymentUri} label="Copy BIP-21 URI" />
        </div>
      )}

      {/* Fulfillment card */}
      {order.fulfillment && (
        <div className="ob-tor-card">
          <h3>{order.fulfillment.content ? "Digital delivery" : "Shipment"}</h3>
          {order.fulfillment.carrier && <p><strong>Carrier:</strong> {order.fulfillment.carrier}</p>}
          {order.fulfillment.tracking && <p><strong>Tracking:</strong> {order.fulfillment.tracking}</p>}
          {order.trackingStatus && <p><strong>Status:</strong> {order.trackingStatus}</p>}
          {order.fulfillment.note && <p>{order.fulfillment.note}</p>}
          {order.fulfillment.content && <pre className="ob-digital-content">{order.fulfillment.content}</pre>}
        </div>
      )}

      {/* Messages card */}
      {order.messages.length > 0 && (
        <div className="ob-tor-card">
          <h3>Messages</h3>
          <div className="ob-tor-messages">
            {order.messages.map((msg) => (
              <div key={msg.id} className={`ob-tor-msg ${msg.from === "buyer" ? "is-mine" : "is-theirs"}`}>
                <strong>{msg.from === "buyer" ? "You" : order.sellerName}</strong>
                <p>{msg.text}</p>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Chat input card */}
      <div className="ob-tor-card">
        <h3>Message seller</h3>
        <div className="ob-tor-chat-input">
          <textarea
            className="ob-textarea"
            value={chatDraft}
            onChange={(e) => setChatDraft(e.target.value)}
            placeholder="Message seller…"
            rows={2}
          />
          <button className="ob-primary" type="button" disabled={busy || !chatDraft.trim()} onClick={() => void sendMessage()}>
            Send message
          </button>
        </div>
      </div>

      <p className="ob-meta">
        Pay directly from any wallet. No app download required. Keep your 12-word receipt code to check order status later.
      </p>
    </section>
  );
}
