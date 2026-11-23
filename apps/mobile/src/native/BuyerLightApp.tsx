import { useEffect, useMemo, useState } from "react";
import { Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import * as Clipboard from "expo-clipboard";

import type { ListingHeader } from "../catalog.ts";
import {
  buildOfflineDisputeBundle,
  type LightOrderRecord,
  type OfflineDisputePayload,
} from "../buyerLight/contracts.ts";
import {
  confirmLightOrderComplete,
  fetchVerifiedStore,
  pullLightOrderStatus,
  sendLightOrderMessage,
  submitLightOrder,
  type VerifiedPublicStore,
} from "../buyerLight/client.ts";
import {
  buyerSeedFromPhrase,
  DEFAULT_BUYER_NETWORK,
  derivePayoutAddress,
  generateBuyerPhrase,
  isValidBuyerPhrase,
  normalizeBuyerPhrase,
  type BuyerSeed,
} from "../buyerLight/keys.ts";
import { Qr } from "../NativePayments.tsx";
import { nativeBuyerLightStore, nativeLightFetcher } from "./buyerLightStorage.ts";
import { ago, Button, Card, formatBtc, LINE, MUTED, NAVY, Notice, Row, SOFT, ui } from "./ui.tsx";

type LightTab = "shop" | "orders" | "seed";

/**
 * Signed-app-code buyer-only light mode (native app only; web/Tor Browser
 * stays strictly browse-only).
 *
 * Lets a buyer without a home node or 24/7 Tor mailbox:
 * 1. Create or restore a standard 12-word BIP-39 recovery seed.
 * 2. Open and cryptographically verify a seller's signed store & listings.
 * 3. Place pull-mode orders, fund via QR/BIP-21 from any external wallet,
 *    and pull sealed order/tracking/chat/return updates on demand.
 * 4. Retain signed contracts locally to build a direct-to-moderator dispute
 *    even if the seller disappears offline.
 */
export function BuyerLightApp({ onPairHomeNode }: { onPairHomeNode: () => void }) {
  if (Platform.OS === "web") {
    return (
      <ScrollView contentContainerStyle={ui.screen}>
        <Notice text="Buyer-only checkout runs only in the signed OpenBazaar app, never in seller-served web pages." tone="error" />
      </ScrollView>
    );
  }

  const [phrase, setPhrase] = useState<string | null>(null);
  const [booting, setBooting] = useState(true);
  const [orders, setOrders] = useState<LightOrderRecord[]>([]);
  const [tab, setTab] = useState<LightTab>("shop");
  const [selectedOrderId, setSelectedOrderId] = useState<string | null>(null);

  useEffect(() => {
    void Promise.all([nativeBuyerLightStore.loadPhrase(), nativeBuyerLightStore.loadOrders()]).then(
      ([savedPhrase, savedOrders]) => {
        setPhrase(savedPhrase && isValidBuyerPhrase(savedPhrase) ? savedPhrase : null);
        setOrders(savedOrders);
        setBooting(false);
      },
    );
  }, []);

  const seed = useMemo<BuyerSeed | null>(() => {
    if (!phrase) return null;
    try {
      return buyerSeedFromPhrase(phrase, DEFAULT_BUYER_NETWORK);
    } catch {
      return null;
    }
  }, [phrase]);

  const persistOrders = async (next: LightOrderRecord[]) => {
    setOrders(next);
    await nativeBuyerLightStore.saveOrders(next);
  };

  const upsertOrder = async (record: LightOrderRecord) => {
    const next = [record, ...orders.filter((item) => item.id !== record.id)];
    await persistOrders(next);
  };

  if (booting) {
    return (
      <View style={styles.center}>
        <Text style={ui.muted}>Loading buyer-only mode…</Text>
      </View>
    );
  }

  if (!seed || !phrase) {
    return (
      <SeedSetup
        onReady={async (chosen) => {
          const clean = normalizeBuyerPhrase(chosen);
          await nativeBuyerLightStore.savePhrase(clean);
          setPhrase(clean);
        }}
        onPairHomeNode={onPairHomeNode}
      />
    );
  }

  const selectedOrder = selectedOrderId ? orders.find((item) => item.id === selectedOrderId) ?? null : null;

  return (
    <View style={styles.fill}>
      <View style={styles.banner}>
        <Text style={styles.bannerText}>
          Buyer-only mode · {seed.network === "testnet4" ? "Testnet4 (test coins, no value)" : "Mainnet"} · No home node required
        </Text>
      </View>
      <View style={styles.navBar}>
        {(
          [
            ["shop", "Shop"],
            ["orders", `Orders (${orders.length})`],
            ["seed", "Recovery seed"],
          ] as const
        ).map(([id, label]) => (
          <Pressable
            key={id}
            accessibilityRole="tab"
            accessibilityState={{ selected: tab === id }}
            onPress={() => {
              setTab(id);
              setSelectedOrderId(null);
            }}
            style={[styles.navTab, tab === id && styles.navTabOn]}
          >
            <Text style={[styles.navTabText, tab === id && styles.navTabTextOn]}>{label}</Text>
          </Pressable>
        ))}
      </View>

      <View style={styles.fill}>
        {tab === "shop" ? (
          <LightShopView
            seed={seed}
            onOrderPlaced={async (order) => {
              await upsertOrder(order);
              setSelectedOrderId(order.id);
              setTab("orders");
            }}
          />
        ) : tab === "orders" ? (
          selectedOrder ? (
            <LightOrderDetail
              order={selectedOrder}
              seed={seed}
              onUpdated={upsertOrder}
              onBack={() => setSelectedOrderId(null)}
            />
          ) : (
            <LightOrdersList orders={orders} onSelect={(id) => setSelectedOrderId(id)} />
          )
        ) : (
          <SeedBackupView
            phrase={phrase}
            seed={seed}
            onPairHomeNode={onPairHomeNode}
            onReset={async () => {
              await nativeBuyerLightStore.clearAll();
              setPhrase(null);
              setOrders([]);
            }}
          />
        )}
      </View>
    </View>
  );
}

function SeedSetup({ onReady, onPairHomeNode }: { onReady: (phrase: string) => Promise<void>; onPairHomeNode: () => void }) {
  const [mode, setMode] = useState<"choose" | "create" | "restore">("choose");
  const [generated, setGenerated] = useState<string>("");
  const [backedUp, setBackedUp] = useState(false);
  const [restoreText, setRestoreText] = useState("");
  const [error, setError] = useState<string | null>(null);

  if (mode === "create") {
    const words = generated.split(" ");
    return (
      <ScrollView contentContainerStyle={ui.screen}>
        <Text style={ui.title}>Write down your 12 recovery words</Text>
        <Text style={ui.subtitle}>
          These 12 words deterministically derive all of your buyer order, escrow, and payout keys. If you lose this phone, these words recover your orders and escrow control.
        </Text>
        <Card title="12-word BIP 39 recovery phrase">
          <View style={styles.wordGrid}>
            {words.map((word, index) => (
              <Text key={`${index}-${word}`} style={styles.wordChip} selectable>
                {index + 1}. {word}
              </Text>
            ))}
          </View>
        </Card>
        <Pressable
          accessibilityRole="checkbox"
          accessibilityState={{ checked: backedUp }}
          onPress={() => setBackedUp((v) => !v)}
          style={styles.checkRow}
        >
          <View style={[styles.checkbox, backedUp && styles.checkboxOn]} />
          <Text style={ui.subtitle}>I wrote down all 12 words in order.</Text>
        </Pressable>
        <Button label="Continue to buyer-only mode" disabled={!backedUp} onPress={() => void onReady(generated)} />
        <Button label="Back" kind="plain" onPress={() => setMode("choose")} />
      </ScrollView>
    );
  }

  if (mode === "restore") {
    return (
      <ScrollView contentContainerStyle={ui.screen} keyboardShouldPersistTaps="handled">
        <Text style={ui.title}>Restore from 12 recovery words</Text>
        <Text style={ui.subtitle}>
          Enter your 12-word BIP 39 recovery phrase to restore your deterministic buyer order and escrow keys.
        </Text>
        {error ? <Notice text={error} tone="error" /> : null}
        <TextInput
          style={[ui.input, styles.multiline]}
          value={restoreText}
          onChangeText={setRestoreText}
          placeholder="twelve space-separated BIP 39 words"
          autoCapitalize="none"
          autoCorrect={false}
          multiline
          accessibilityLabel="12-word recovery phrase"
        />
        <Button
          label="Restore keys"
          disabled={!restoreText.trim()}
          onPress={() => {
            if (!isValidBuyerPhrase(restoreText)) {
              setError("That is not a valid 12-word BIP 39 recovery phrase.");
              return;
            }
            setError(null);
            void onReady(restoreText);
          }}
        />
        <Button label="Back" kind="plain" onPress={() => setMode("choose")} />
      </ScrollView>
    );
  }

  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Buy without a home node</Text>
      <Text style={ui.subtitle}>
        Verify listings and escrow addresses locally in signed app code, fund orders via QR from any wallet, pull sealed seller updates on demand, and retain signed contracts for disputes.
      </Text>
      <Card title="12-word recovery seed">
        <Text style={ui.muted}>
          Your keys stay on this device and are recoverable from a standard 12-word BIP 39 seed. No store hosting or 24/7 Tor mailbox is needed.
        </Text>
        <Button
          label="Create new 12-word recovery seed"
          onPress={() => {
            setGenerated(generateBuyerPhrase());
            setBackedUp(false);
            setMode("create");
          }}
        />
        <Button label="Restore existing 12-word seed" kind="plain" onPress={() => setMode("restore")} />
      </Card>
      <Button label="Pair with a home node instead" kind="plain" onPress={onPairHomeNode} />
    </ScrollView>
  );
}

function LightShopView({
  seed,
  onOrderPlaced,
}: {
  seed: BuyerSeed;
  onOrderPlaced: (order: LightOrderRecord) => Promise<void>;
}) {
  const [urlDraft, setUrlDraft] = useState("");
  const [store, setStore] = useState<VerifiedPublicStore | null>(null);
  const [selected, setSelected] = useState<ListingHeader | null>(null);
  const [quantityText, setQuantityText] = useState("1");
  const [shipTo, setShipTo] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const openStore = async () => {
    if (!urlDraft.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const verified = await fetchVerifiedStore(urlDraft, nativeLightFetcher);
      setStore(verified);
      setSelected(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not open that store.");
    } finally {
      setBusy(false);
    }
  };

  const placeOrder = async () => {
    if (!store || !selected || busy) return;
    const quantity = Number.parseInt(quantityText, 10);
    if (!Number.isInteger(quantity) || quantity < 1 || quantity > 99) {
      setError("Enter a whole quantity between 1 and 99.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const record = await submitLightOrder({
        seed,
        store,
        listing: selected,
        quantity,
        shipTo,
        settlement: "direct",
        rail: "btc",
        fetcher: nativeLightFetcher,
      });
      await onOrderPlaced(record);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not place order.");
    } finally {
      setBusy(false);
    }
  };

  if (store && selected) {
    return (
      <ScrollView contentContainerStyle={ui.screen} keyboardShouldPersistTaps="handled">
        <Button label="Back to store listings" kind="plain" onPress={() => setSelected(null)} />
        <Text style={ui.title}>{selected.title}</Text>
        <Text style={ui.muted}>
          {selected.seller} · {selected.priceSats.toLocaleString("en-US")} sats ({formatBtc(selected.priceSats)})
        </Text>
        {selected.description ? <Text style={ui.subtitle}>{selected.description}</Text> : null}
        <Card title="Pull-mode checkout">
          <Text style={ui.muted}>
            Your order is sealed locally to the store's verified mailbox key. You will get a verified Bitcoin payment address & QR code to pay from any wallet.
          </Text>
          <Text style={ui.rowLabel}>Quantity</Text>
          <TextInput
            style={ui.input}
            value={quantityText}
            onChangeText={setQuantityText}
            keyboardType="number-pad"
            accessibilityLabel="Quantity"
          />
          <Text style={ui.rowLabel}>{selected.kind === "physical" ? "Shipping address" : "Delivery note"}</Text>
          <TextInput
            style={[ui.input, styles.multiline]}
            value={shipTo}
            onChangeText={setShipTo}
            multiline
            placeholder={selected.kind === "physical" ? "Name, street, city, region, postal code, country" : "How should the seller deliver this?"}
            accessibilityLabel={selected.kind === "physical" ? "Shipping address" : "Delivery note"}
          />
          {error ? <Notice text={error} tone="error" /> : null}
          <Button label="Seal & place order" busy={busy} onPress={() => void placeOrder()} />
        </Card>
      </ScrollView>
    );
  }

  return (
    <ScrollView contentContainerStyle={ui.screen} keyboardShouldPersistTaps="handled">
      <Card title="Open a verified store">
        <Text style={ui.muted}>
          Paste a seller's .onion or https:// store address. Signatures and hostile-text filters run locally inside this app.
        </Text>
        <TextInput
          style={ui.input}
          value={urlDraft}
          onChangeText={setUrlDraft}
          placeholder="http://….onion or https://…"
          autoCapitalize="none"
          autoCorrect={false}
          accessibilityLabel="Store address"
        />
        {error ? <Notice text={error} tone="error" /> : null}
        <Button label="Open & verify store" busy={busy} disabled={!urlDraft.trim()} onPress={() => void openStore()} />
      </Card>
      {store ? (
        <Card title={`${store.catalog.store.name} (${store.catalog.listings.length} verified listings)`}>
          {store.catalog.listings.length === 0 ? (
            <Text style={ui.muted}>No verified listings in this store.</Text>
          ) : (
            store.catalog.listings.map((item) => (
              <Pressable
                key={item.hash}
                accessibilityRole="button"
                onPress={() => {
                  setSelected(item);
                  setQuantityText("1");
                  setError(null);
                }}
                style={styles.listingRow}
              >
                <Text style={styles.listingTitle}>{item.title}</Text>
                <Text style={ui.muted}>
                  {item.priceSats.toLocaleString("en-US")} sats · {item.kind === "digital" ? "Digital" : `Ships from ${item.shipFrom}`}
                </Text>
              </Pressable>
            ))
          )}
        </Card>
      ) : null}
    </ScrollView>
  );
}

function LightOrdersList({ orders, onSelect }: { orders: LightOrderRecord[]; onSelect: (id: string) => void }) {
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Your local orders</Text>
      {orders.length === 0 ? (
        <Text style={ui.muted}>No orders yet. Open a store in the Shop tab to place a pull-mode order.</Text>
      ) : (
        orders.map((order) => (
          <Pressable key={order.id} accessibilityRole="button" onPress={() => onSelect(order.id)} style={styles.orderCard}>
            <Text style={styles.listingTitle}>{order.sellerName}</Text>
            <Text style={ui.subtitle}>
              {order.quantity} × {order.title}
            </Text>
            <Row label="Status" value={order.status} />
            <Row label="Total" value={`${order.totalSats.toLocaleString("en-US")} sats`} />
          </Pressable>
        ))
      )}
    </ScrollView>
  );
}

function LightOrderDetail({
  order,
  seed,
  onUpdated,
  onBack,
}: {
  order: LightOrderRecord;
  seed: BuyerSeed;
  onUpdated: (order: LightOrderRecord) => Promise<void>;
  onBack: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [chatDraft, setChatDraft] = useState("");
  const [disputeMemo, setDisputeMemo] = useState("");
  const [disputeBundle, setDisputeBundle] = useState<OfflineDisputePayload | null>(null);

  const copyValue = async (label: string, value: string) => {
    await Clipboard.setStringAsync(value);
    setCopied(label);
    setTimeout(() => setCopied(null), 1800);
  };

  const pullStatus = async () => {
    setBusy(true);
    setError(null);
    try {
      const updated = await pullLightOrderStatus(order, seed, nativeLightFetcher);
      await onUpdated(updated);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not pull order status.");
    } finally {
      setBusy(false);
    }
  };

  const sendMessage = async () => {
    if (!chatDraft.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const updated = await sendLightOrderMessage(order, seed, chatDraft, nativeLightFetcher);
      setChatDraft("");
      await onUpdated(updated);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not send message.");
    } finally {
      setBusy(false);
    }
  };

  const confirmArrival = async () => {
    setBusy(true);
    setError(null);
    try {
      const updated = await confirmLightOrderComplete(order, seed, nativeLightFetcher);
      await onUpdated(updated);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not confirm order arrival.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <ScrollView contentContainerStyle={ui.screen} keyboardShouldPersistTaps="handled">
      <Button label="Back to orders" kind="plain" onPress={onBack} />
      <Text style={ui.title}>{order.sellerName}</Text>
      <Text style={ui.subtitle}>
        {order.quantity} × {order.title}
      </Text>
      {error ? <Notice text={error} tone="error" /> : null}
      {order.declinedReason ? <Notice text={order.declinedReason} tone="error" /> : null}

      <Card title="Order status (pull-based)">
        <Row label="Status" value={order.status} />
        <Row label="Total" value={`${order.totalSats.toLocaleString("en-US")} sats (${formatBtc(order.totalSats)})`} />
        <Row label="Received" value={`${order.receivedSats.toLocaleString("en-US")} sats (${order.confirmations} confs)`} />
        <Row label="Last pulled" value={ago(order.lastPulledAt)} />
        <Button label="Refresh status from seller" busy={busy} onPress={() => void pullStatus()} />
        {order.status === "Shipped" ? (
          <Button label="Confirm it arrived" kind="plain" busy={busy} onPress={() => void confirmArrival()} />
        ) : null}
      </Card>

      {order.paymentAddress && order.paymentUri ? (
        <Card title="Pay from any external wallet">
          <Text style={ui.muted}>
            Scan this BIP-21 QR code or copy the verified {order.network} address to fund from any wallet you already use.
          </Text>
          <Qr value={order.paymentUri} />
          <Row label="Address" value={order.paymentAddress} />
          <Button
            label={copied === "address" ? "Copied address" : "Copy payment address"}
            kind="plain"
            onPress={() => void copyValue("address", order.paymentAddress!)}
          />
          <Button
            label={copied === "uri" ? "Copied BIP-21 URI" : "Copy BIP-21 URI"}
            kind="plain"
            onPress={() => void copyValue("uri", order.paymentUri!)}
          />
        </Card>
      ) : null}

      {order.fulfillment ? (
        <Card title={order.fulfillment.content ? "Digital delivery" : "Shipment"}>
          {order.fulfillment.carrier ? <Row label="Carrier" value={order.fulfillment.carrier} /> : null}
          {order.fulfillment.tracking ? <Row label="Tracking" value={order.fulfillment.tracking} /> : null}
          {order.trackingStatus ? <Row label="Tracking status" value={order.trackingStatus} /> : null}
          {order.fulfillment.note ? <Text style={ui.subtitle}>{order.fulfillment.note}</Text> : null}
          {order.fulfillment.content ? <Text style={ui.subtitle} selectable>{order.fulfillment.content}</Text> : null}
        </Card>
      ) : null}

      <Card title="Messages">
        {order.messages.length === 0 ? (
          <Text style={ui.muted}>No messages yet.</Text>
        ) : (
          order.messages.map((msg) => (
            <View key={msg.id} style={styles.messageRow}>
              <Text style={ui.rowLabel}>{msg.from === "buyer" ? "You" : order.sellerName}</Text>
              <Text style={ui.subtitle}>{msg.text}</Text>
            </View>
          ))
        )}
        <TextInput
          style={ui.input}
          value={chatDraft}
          onChangeText={setChatDraft}
          placeholder="Message seller…"
          accessibilityLabel="Message seller"
        />
        <Button label="Send message" kind="plain" busy={busy} disabled={!chatDraft.trim()} onPress={() => void sendMessage()} />
      </Card>

      <Card title="Locally retained signed contract">
        <Text style={ui.muted}>
          Signed store record and listing record are stored locally on this device so you can prove your order terms even if the seller shuts off their node.
        </Text>
        <Row label="Store key" value={`${order.storeId.slice(0, 16)}…`} />
        <Row label="Order drop id" value={order.dropIdHex} />
        {order.escrow ? (
          <>
            <Text style={ui.rowLabel}>Offline-seller dispute (direct to moderator)</Text>
            <TextInput
              style={[ui.input, styles.multiline]}
              value={disputeMemo}
              onChangeText={setDisputeMemo}
              multiline
              placeholder="Describe what went wrong for the moderator"
              accessibilityLabel="Dispute memo"
            />
            <Button
              label="Build signed dispute payload"
              disabled={!disputeMemo.trim()}
              onPress={() => {
                try {
                  setError(null);
                  setDisputeBundle(buildOfflineDisputeBundle(order, seed, disputeMemo));
                } catch (err) {
                  setError(err instanceof Error ? err.message : "Could not build dispute payload.");
                }
              }}
            />
            {disputeBundle ? (
              <Notice
                text={`Dispute payload signed for moderator at ${disputeBundle.moderatorUrl} (verified escrow ${disputeBundle.verifiedFundingAddress}).`}
              />
            ) : null}
          </>
        ) : null}
      </Card>
    </ScrollView>
  );
}

function SeedBackupView({
  phrase,
  seed,
  onPairHomeNode,
  onReset,
}: {
  phrase: string;
  seed: BuyerSeed;
  onPairHomeNode: () => void;
  onReset: () => Promise<void>;
}) {
  const [reveal, setReveal] = useState(false);
  const payout0 = useMemo(() => derivePayoutAddress(seed, 0), [seed]);
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Recovery seed</Text>
      <Card title="Deterministic buyer identity">
        <Row label="Network" value={seed.network} />
        <Row label="First Taproot payout" value={payout0} />
        <Text style={ui.muted}>
          Keep your 12-word phrase offline. Anyone with these 12 words can control your buyer order and escrow keys.
        </Text>
        {reveal ? (
          <Text style={styles.seedReveal} selectable>
            {phrase}
          </Text>
        ) : null}
        <Button label={reveal ? "Hide 12 words" : "Reveal 12 recovery words"} kind="plain" onPress={() => setReveal((v) => !v)} />
      </Card>
      <Button label="Pair with a home node" kind="plain" onPress={onPairHomeNode} />
      <Button label="Reset buyer-only seed on this phone" kind="danger" onPress={() => void onReset()} />
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  center: { flex: 1, alignItems: "center", justifyContent: "center", padding: 24 },
  banner: { backgroundColor: "#fff0c9", paddingHorizontal: 14, paddingVertical: 8, borderBottomWidth: 1, borderBottomColor: LINE },
  bannerText: { color: "#714b0a", fontSize: 13, fontWeight: "600" },
  navBar: { flexDirection: "row", borderBottomWidth: 1, borderBottomColor: LINE, backgroundColor: "#fff" },
  navTab: { flex: 1, minHeight: 48, alignItems: "center", justifyContent: "center", paddingHorizontal: 8 },
  navTabOn: { borderBottomWidth: 2, borderBottomColor: NAVY },
  navTabText: { fontSize: 14, fontWeight: "600", color: MUTED },
  navTabTextOn: { color: NAVY },
  wordGrid: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
  wordChip: { backgroundColor: SOFT, color: NAVY, paddingHorizontal: 10, paddingVertical: 6, borderRadius: 8, fontSize: 15, fontWeight: "600" },
  checkRow: { flexDirection: "row", alignItems: "center", gap: 10 },
  checkbox: { width: 20, height: 20, borderRadius: 4, borderWidth: 1, borderColor: MUTED },
  checkboxOn: { backgroundColor: NAVY, borderColor: NAVY },
  multiline: { minHeight: 88, textAlignVertical: "top" },
  listingRow: { paddingVertical: 12, borderTopWidth: 1, borderTopColor: LINE, gap: 4 },
  listingTitle: { fontSize: 17, fontWeight: "600", color: NAVY },
  orderCard: { backgroundColor: "#fff", borderRadius: 12, padding: 16, borderWidth: 1, borderColor: LINE, gap: 8 },
  messageRow: { paddingVertical: 8, borderTopWidth: 1, borderTopColor: LINE, gap: 2 },
  seedReveal: { fontFamily: "Menlo", fontSize: 15, lineHeight: 24, color: NAVY, backgroundColor: SOFT, padding: 12, borderRadius: 8 },
});
