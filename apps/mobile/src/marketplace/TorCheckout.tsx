// Tor Browser direct-pay checkout for buyer-only pull-mode orders.
//
// Runs entirely inside the moderator-served shop page so it must work
// cross-origin: sealed orders go to `POST {vendorUrl}/mail` and status
// pulls go to `POST {vendorUrl}/mail/pull`. The sale_gate on the vendor
// node allows these two routes with CORS.
//
// Key management uses the same deterministic BIP-39 seed derivation as
// the native BuyerLightApp, stored in localStorage via webStore.ts.

import { useEffect, useMemo, useRef, useState } from "react";
import type { ListingHeader } from "../catalog.ts";
import type { LightOrderRecord } from "../buyerLight/contracts.ts";
import { applySellerUpdate, buildBip21Uri } from "../buyerLight/contracts.ts";
import {
  fetchVerifiedStore,
  pullLightOrderStatus,
  sendLightOrderMessage,
  submitLightOrder,
  confirmLightOrderComplete,
  type VerifiedPublicStore,
} from "../buyerLight/client.ts";
import {
  buyerSeedFromPhrase,
  DEFAULT_BUYER_NETWORK,
  generateBuyerPhrase,
  isValidBuyerPhrase,
  normalizeBuyerPhrase,
  type BuyerSeed,
} from "../buyerLight/keys.ts";
import { createWebBuyerStore } from "../buyerLight/webStore.ts";
import { CopyButton } from "./CopyAddress.tsx";
import { PageHeading } from "./PageHeading.tsx";

const store = createWebBuyerStore();

type Phase = "seed" | "form" | "placed";

export function TorCheckout({
  listing,
  vendorUrl,
  onBack,
  backLabel,
}: {
  listing: ListingHeader;
  vendorUrl: string;
  onBack: () => void;
  backLabel: string;
}) {
  const [phrase, setPhrase] = useState<string | null>(null);
  const [booting, setBooting] = useState(true);
  const [order, setOrder] = useState<LightOrderRecord | null>(null);
  const [phase, setPhase] = useState<Phase>("seed");
  const [verifiedStore, setVerifiedStore] = useState<VerifiedPublicStore | null>(null);
  const [storeError, setStoreError] = useState("");

  useEffect(() => {
    void store.loadPhrase().then((saved) => {
      if (saved && isValidBuyerPhrase(saved)) {
        setPhrase(saved);
        setPhase("form");
      }
      setBooting(false);
    });
    void fetchVerifiedStore(vendorUrl).then(setVerifiedStore).catch((err) => {
      setStoreError(err instanceof Error ? err.message : "Could not verify the seller's store.");
    });
  }, [vendorUrl]);

  const seed = useMemo<BuyerSeed | null>(() => {
    if (!phrase) return null;
    try {
      return buyerSeedFromPhrase(phrase, DEFAULT_BUYER_NETWORK);
    } catch {
      return null;
    }
  }, [phrase]);

  if (booting || (!verifiedStore && !storeError)) {
    return (
      <section className="ob-checkout">
        <p className="ob-meta">Loading checkout…</p>
      </section>
    );
  }

  if (storeError) {
    return (
      <section className="ob-checkout">
        <button className="ob-back" type="button" onClick={onBack}>{backLabel}</button>
        <p className="ob-error">{storeError}</p>
      </section>
    );
  }

  if (phase === "seed" || !seed || !phrase) {
    return (
      <SeedStep
        onReady={(chosen) => {
          const clean = normalizeBuyerPhrase(chosen);
          void store.savePhrase(clean);
          setPhrase(clean);
          setPhase("form");
        }}
        onBack={onBack}
        backLabel={backLabel}
      />
    );
  }

  if (phase === "placed" && order) {
    return (
      <OrderStatus
        order={order}
        seed={seed}
        vendorUrl={vendorUrl}
        onUpdated={(next) => {
          setOrder(next);
          void store.loadOrders().then((all) => {
            void store.saveOrders([next, ...all.filter((o) => o.id !== next.id)]);
          });
        }}
        onBack={onBack}
        backLabel={backLabel}
      />
    );
  }

  return (
    <CheckoutForm
      listing={listing}
      vendorUrl={vendorUrl}
      verifiedStore={verifiedStore!}
      seed={seed}
      phrase={phrase}
      onPlaced={(record) => {
        setOrder(record);
        setPhase("placed");
        void store.loadOrders().then((all) => {
          void store.saveOrders([record, ...all.filter((o) => o.id !== record.id)]);
        });
      }}
      onBack={onBack}
      backLabel={backLabel}
    />
  );
}

function SeedStep({
  onReady,
  onBack,
  backLabel,
}: {
  onReady: (phrase: string) => void;
  onBack: () => void;
  backLabel: string;
}) {
  const [mode, setMode] = useState<"choose" | "create" | "restore">("choose");
  const [generated, setGenerated] = useState("");
  const [backedUp, setBackedUp] = useState(false);
  const [restoreText, setRestoreText] = useState("");
  const [error, setError] = useState("");

  if (mode === "create") {
    const words = generated.split(" ");
    return (
      <section className="ob-checkout">
        <button className="ob-back" type="button" onClick={() => setMode("choose")}>
          Back
        </button>
        <PageHeading
          title="Write down your 12 recovery words"
          description="These 12 words recover your orders and receipt codes. Keep them offline."
        />
        <div className="ob-seed-grid">
          {words.map((w, i) => (
            <span key={i} className="ob-seed-word">
              {i + 1}. {w}
            </span>
          ))}
        </div>
        <label className="ob-field">
          <input
            type="checkbox"
            checked={backedUp}
            onChange={() => setBackedUp((v) => !v)}
          />{" "}
          I wrote down all 12 words in order.
        </label>
        <button
          className="ob-primary"
          disabled={!backedUp}
          onClick={() => onReady(generated)}
        >
          Continue to checkout
        </button>
      </section>
    );
  }

  if (mode === "restore") {
    return (
      <section className="ob-checkout">
        <button className="ob-back" type="button" onClick={() => setMode("choose")}>
          Back
        </button>
        <PageHeading
          title="Restore from 12 recovery words"
          description="Enter the 12-word receipt code from a previous order to restore your buyer identity."
        />
        {error && <p className="ob-error">{error}</p>}
        <textarea
          className="ob-textarea"
          value={restoreText}
          onChange={(e) => setRestoreText(e.target.value)}
          placeholder="twelve space-separated BIP 39 words"
          rows={3}
        />
        <button
          className="ob-primary"
          disabled={!restoreText.trim()}
          onClick={() => {
            if (!isValidBuyerPhrase(restoreText)) {
              setError("That is not a valid 12-word recovery phrase.");
              return;
            }
            setError("");
            onReady(restoreText);
          }}
        >
          Restore & continue
        </button>
      </section>
    );
  }

  return (
    <section className="ob-checkout">
      <button className="ob-back" type="button" onClick={onBack}>
        {backLabel}
      </button>
      <PageHeading
        title="Direct pay checkout"
        description="Pay directly from any Bitcoin wallet. No app download required. Keep your 12-word receipt code to check order status later."
      />
      <div className="ob-empty-actions">
        <button
          className="ob-primary"
          onClick={() => {
            setGenerated(generateBuyerPhrase());
            setBackedUp(false);
            setMode("create");
          }}
        >
          Create new receipt code
        </button>
        <button className="ob-secondary" onClick={() => setMode("restore")}>
          Restore existing receipt code
        </button>
      </div>
    </section>
  );
}

function CheckoutForm({
  listing,
  vendorUrl,
  verifiedStore,
  seed,
  phrase,
  onPlaced,
  onBack,
  backLabel,
}: {
  listing: ListingHeader;
  vendorUrl: string;
  verifiedStore: VerifiedPublicStore;
  seed: BuyerSeed;
  phrase: string;
  onPlaced: (record: LightOrderRecord) => void;
  onBack: () => void;
  backLabel: string;
}) {
  const [quantity, setQuantity] = useState(1);
  const [shipTo, setShipTo] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const record = await submitLightOrder({
        seed,
        store: verifiedStore,
        listing,
        quantity,
        shipTo,
        settlement: "direct",
        rail: "btc",
      });
      onPlaced(record);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not place order.");
    } finally {
      setBusy(false);
    }
  };

  const words = phrase.split(" ");

  return (
    <section className="ob-checkout">
      <button className="ob-back" type="button" onClick={onBack}>
        {backLabel}
      </button>
      <PageHeading title={listing.title} description={`${listing.priceSats.toLocaleString()} sats · ${listing.seller}`} />

      <div className="ob-checkout-receipt">
        <h3>Your 12-word receipt code</h3>
        <p className="ob-meta">
          Save these 12 words to check order status and tracking later.
        </p>
        <div className="ob-seed-grid">
          {words.map((w, i) => (
            <span key={i} className="ob-seed-word">
              {i + 1}. {w}
            </span>
          ))}
        </div>
        <CopyButton value={phrase} label="Copy receipt code" />
      </div>

      <label className="ob-field">
        <span>Quantity</span>
        <input
          type="number"
          min={1}
          max={99}
          value={quantity}
          onChange={(e) => setQuantity(Math.max(1, Math.min(99, Number(e.target.value) || 1)))}
        />
      </label>
      <label className="ob-field">
        <span>{listing.kind === "physical" ? "Shipping address" : "Delivery note"}</span>
        <textarea
          className="ob-textarea"
          value={shipTo}
          onChange={(e) => setShipTo(e.target.value)}
          placeholder={
            listing.kind === "physical"
              ? "Name, street, city, region, postal code, country"
              : "How should the seller deliver this?"
          }
          rows={3}
        />
      </label>
      {error && <p className="ob-error">{error}</p>}
      <button className="ob-primary" disabled={busy || !shipTo.trim()} onClick={() => void submit()}>
        {busy ? "Placing order…" : "Place order & get payment address"}
      </button>
      <p className="ob-meta">
        Your order is sealed and sent directly to the seller's node. You'll receive a Bitcoin payment address to pay from any wallet.
      </p>
    </section>
  );
}

function OrderStatus({
  order,
  seed,
  vendorUrl,
  onUpdated,
  onBack,
  backLabel,
}: {
  order: LightOrderRecord;
  seed: BuyerSeed;
  vendorUrl: string;
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

  return (
    <section className="ob-checkout">
      <button className="ob-back" type="button" onClick={onBack}>
        {backLabel}
      </button>
      <PageHeading title={`Order: ${order.title}`} description={`${order.sellerName} · ${order.status}`} />
      {error && <p className="ob-error">{error}</p>}
      {order.declinedReason && <p className="ob-error">{order.declinedReason}</p>}

      <div className="ob-order-facts">
        <dl>
          <dt>Status</dt><dd>{order.status}</dd>
          <dt>Total</dt><dd>{order.totalSats.toLocaleString()} sats</dd>
          {order.receivedSats > 0 && <><dt>Received</dt><dd>{order.receivedSats.toLocaleString()} sats ({order.confirmations} confs)</dd></>}
        </dl>
        <button className="ob-secondary" disabled={busy} onClick={() => void pullStatus()}>
          {busy ? "Refreshing…" : "Refresh status from seller"}
        </button>
        {order.status === "Shipped" && (
          <button className="ob-primary" disabled={busy} onClick={() => void confirmArrival()}>
            Confirm it arrived
          </button>
        )}
      </div>

      {order.paymentAddress && order.paymentUri && (
        <div className="ob-payment-box">
          <h3>Pay from any Bitcoin wallet</h3>
          <p className="ob-meta">
            Send exactly <strong>{order.totalSats.toLocaleString()} sats</strong> to the address below.
            Scan the QR code or copy the address.
          </p>
          <div className="ob-qr-placeholder">
            <img
              src={`https://api.qrserver.com/v1/create-qr-code/?size=200x200&data=${encodeURIComponent(order.paymentUri)}`}
              alt="Payment QR code"
              width={200}
              height={200}
            />
          </div>
          <div className="ob-copy-address">
            <code>{order.paymentAddress}</code>
            <CopyButton value={order.paymentAddress} label="Copy address" />
          </div>
          <div className="ob-copy-address">
            <code className="ob-meta">{order.paymentUri}</code>
            <CopyButton value={order.paymentUri} label="Copy BIP-21 URI" />
          </div>
        </div>
      )}

      {order.fulfillment && (
        <div className="ob-fulfillment-box">
          <h3>{order.fulfillment.content ? "Digital delivery" : "Shipment"}</h3>
          {order.fulfillment.carrier && <p><strong>Carrier:</strong> {order.fulfillment.carrier}</p>}
          {order.fulfillment.tracking && <p><strong>Tracking:</strong> {order.fulfillment.tracking}</p>}
          {order.trackingStatus && <p><strong>Status:</strong> {order.trackingStatus}</p>}
          {order.fulfillment.note && <p>{order.fulfillment.note}</p>}
          {order.fulfillment.content && <pre className="ob-digital-content">{order.fulfillment.content}</pre>}
        </div>
      )}

      {order.messages.length > 0 && (
        <div className="ob-messages">
          <h3>Messages</h3>
          {order.messages.map((msg) => (
            <div key={msg.id} className={`ob-message ob-message-${msg.from}`}>
              <strong>{msg.from === "buyer" ? "You" : order.sellerName}</strong>
              <p>{msg.text}</p>
            </div>
          ))}
        </div>
      )}

      <div className="ob-chat-box">
        <textarea
          className="ob-textarea"
          value={chatDraft}
          onChange={(e) => setChatDraft(e.target.value)}
          placeholder="Message seller…"
          rows={2}
        />
        <button className="ob-secondary" disabled={busy || !chatDraft.trim()} onClick={() => void sendMessage()}>
          Send message
        </button>
      </div>

      <p className="ob-meta">
        Pay directly from any wallet. No app download required. Keep your 12-word receipt code to check order status later.
      </p>
    </section>
  );
}
