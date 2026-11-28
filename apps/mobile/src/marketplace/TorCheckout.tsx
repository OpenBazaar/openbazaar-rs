// Tor Browser direct-pay checkout for buyer-only pull-mode orders.
//
// Runs entirely inside the moderator-served shop page so it must work
// cross-origin: sealed orders go to `POST {vendorUrl}/mail` and status
// pulls go to `POST {vendorUrl}/mail/pull`. The sale_gate on the vendor
// node allows these two routes with CORS.
//
// Key management uses the same deterministic BIP-39 seed derivation as
// the native BuyerLightApp, stored in localStorage via webStore.ts.

import { useEffect, useMemo, useState } from "react";
import type { ListingHeader } from "../catalog.ts";
import type { LightOrderRecord } from "../buyerLight/contracts.ts";
import {
  fetchVerifiedStore,
  nextTorCheckoutId,
  submitLightOrder,
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
import { TorOrderDetail, WordGrid } from "./TorOrderDetail.tsx";

const store = createWebBuyerStore();

type Phase = "seed" | "form" | "placed";

/* ── Root ─────────────────────────────────────────────────────── */

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
      <section className="ob-tor-checkout">
        <p className="ob-meta">Loading checkout…</p>
      </section>
    );
  }

  if (storeError) {
    return (
      <section className="ob-tor-checkout">
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
      <TorOrderDetail
        order={order}
        seed={seed}
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

/* ── Seed step ────────────────────────────────────────────────── */

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
      <section className="ob-tor-checkout">
        <button className="ob-back" type="button" onClick={() => setMode("choose")}>
          Back
        </button>
        <PageHeading
          title="Write down your 12 recovery words"
          description="These 12 words recover your orders and receipt codes. Keep them offline."
        />
        <WordGrid words={words} copyValue={generated} />
        <label className="ob-tor-checkbox-row">
          <input
            type="checkbox"
            checked={backedUp}
            onChange={() => setBackedUp((v) => !v)}
          />
          <span>I wrote down all 12 words in order.</span>
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
      <section className="ob-tor-checkout">
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
    <section className="ob-tor-checkout">
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

/* ── Checkout form ────────────────────────────────────────────── */

function CheckoutForm({
  listing,
  verifiedStore,
  seed,
  phrase,
  onPlaced,
  onBack,
  backLabel,
}: {
  listing: ListingHeader;
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
      const existing = await store.loadOrders();
      const checkoutId = nextTorCheckoutId(existing, listing.storeId);
      const record = await submitLightOrder({
        seed,
        store: verifiedStore,
        listing,
        quantity,
        shipTo,
        settlement: "direct",
        rail: "btc",
        checkoutId,
      });
      onPlaced(record);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not place order.");
    } finally {
      setBusy(false);
    }
  };

  const words = phrase.split(" ");
  const unitPrice = listing.priceSats;
  const total = unitPrice * quantity;

  return (
    <section className="ob-tor-checkout">
      <button className="ob-back" type="button" onClick={onBack}>
        {backLabel}
      </button>
      <PageHeading title={listing.title} description={`${unitPrice.toLocaleString()} sats · ${listing.seller}`} />

      <div className="ob-tor-card">
        <div className="ob-tor-card-header">
          <h3>Your 12-word receipt code</h3>
          <CopyButton value={phrase} label="Copy receipt code" />
        </div>
        <p className="ob-meta">
          Save these 12 words to check order status and tracking later.
        </p>
        <WordGrid words={words} />
      </div>

      <div className="ob-tor-card">
        <h3>Order details</h3>
        <label className="ob-tor-compact-field">
          <span>Quantity</span>
          <input
            className="ob-tor-qty"
            type="number"
            min={1}
            max={99}
            value={quantity}
            onChange={(e) => setQuantity(Math.max(1, Math.min(99, Number(e.target.value) || 1)))}
          />
        </label>
        <div className="ob-tor-order-summary">
          <span>{listing.title}</span>
          <span>{quantity} × {unitPrice.toLocaleString()} sats</span>
          <strong>Total: {total.toLocaleString()} sats</strong>
        </div>
        <label className="ob-tor-compact-field">
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
      </div>

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
