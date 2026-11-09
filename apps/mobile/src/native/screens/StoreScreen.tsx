import { useEffect, useMemo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";

import { parseCatalog, type ContentRating, type ListingHeader, type ShopCatalog } from "../../catalog.ts";
import { collectionValue } from "../../homeNode/replica.ts";
import { purchaseTerms, type AvailabilityDraft } from "../../commerceDraft.ts";
import { couponDraft, type CouponForm } from "../../couponDraft.ts";
import { bulkEditListings, displayNameError, fetchCoupons, saveCoupon, fetchDrafts, updateProfile, type SavedDraft, type StoreCoupon } from "../../seller.ts";
import { isLocked, passphraseError, unlockWallet } from "../../walletLock.ts";
import { ListingPhoto } from "../ListingPhoto.tsx";
import { dismissRefused, queueEdit, sync, useSession } from "../session.ts";
import { Button, Card, formatBtc, LINE, MUTED, NAVY, Notice, Row, SOFT, ui } from "../ui.tsx";
import { ListingEdit } from "./ListingEdit.tsx";

const RATINGS: Exclude<ContentRating, "unrated">[] = ["general", "sensitive", "adult"];

function useCatalog(): { catalog: ShopCatalog | null; error: string | null } {
  const { replica } = useSession();
  return useMemo(() => {
    const value = collectionValue(replica, "catalog");
    if (!value) return { catalog: null, error: null };
    try {
      return { catalog: parseCatalog(value), error: null };
    } catch (err) {
      return { catalog: null, error: err instanceof Error ? err.message : "The catalog could not be read." };
    }
  }, [replica]);
}

function Listing({ listing, queued, canEdit, ownWithoutId, onRate, onEdit }: { listing: ListingHeader; queued: boolean; canEdit: boolean; ownWithoutId: boolean; onRate: (rating: Exclude<ContentRating, "unrated">) => void; onEdit: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <View style={styles.listing}>
      <Pressable accessibilityRole="button" onPress={() => setOpen(!open)} style={styles.listingHead}>
        <ListingPhoto hash={listing.images[0]} title={listing.title} style={styles.thumb} />
        <View style={styles.listingText}>
          <Text style={styles.listingTitle}>{listing.title}</Text>
          <Text style={ui.muted}>
            {formatBtc(listing.priceSats)} · {listing.stock} in stock · {listing.contentRating}
            {queued ? " · change waiting" : ""}
          </Text>
        </View>
        <Text style={styles.chevron}>{open ? "–" : "+"}</Text>
      </Pressable>
      {open ? (
        <View style={styles.rating}>
          <Text style={ui.muted}>Content rating (your store signs it again):</Text>
          <View style={styles.choices}>
            {RATINGS.map(rating => (
              <Pressable key={rating} accessibilityRole="button" onPress={() => onRate(rating)} style={[styles.choice, listing.contentRating === rating && styles.choiceOn]}>
                <Text style={[styles.choiceText, listing.contentRating === rating && styles.choiceTextOn]}>{rating}</Text>
              </Pressable>
            ))}
          </View>
          {canEdit ? <Button label="Edit listing" kind="plain" onPress={onEdit} /> : null}
          {ownWithoutId ? <Text style={ui.muted}>This listing has no stable id, so the node cannot update it.</Text> : null}
        </View>
      ) : null}
    </View>
  );
}

function CouponEditor({ onDone }: { onDone: () => void }) {
  const [coupons, setCoupons] = useState<StoreCoupon[]>([]);
  const [form, setForm] = useState<CouponForm>({ code: "", kind: "percent", amount: "", currency: "USD", minSubtotal: "", startsOn: "", endsOn: "", maxUses: "", listings: [] });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [needsUnlock, setNeedsUnlock] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  useEffect(() => {
    void fetchCoupons().then(setCoupons).catch(() => setCoupons([]));
  }, []);
  async function save() {
    const body = couponDraft(form);
    if (typeof body === "string") {
      setError(body);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (needsUnlock) {
        await unlockWallet(passphrase);
        setPassphrase("");
        setNeedsUnlock(false);
      }
      await saveCoupon(body);
      setCoupons(await fetchCoupons());
      setForm({ code: "", kind: "percent", amount: "", currency: "USD", minSubtotal: "", startsOn: "", endsOn: "", maxUses: "", listings: [] });
      onDone();
    } catch (err) {
      if (isLocked(err)) {
        setNeedsUnlock(true);
        setError("Unlock the wallet on your node so it can sign this coupon.");
      } else setError(passphraseError(err, "The coupon was not signed."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Card title="Coupons">
      <Text style={ui.muted}>A code the store signs. Buyers type it at checkout. It is not part of a listing.</Text>
      <TextInput accessibilityLabel="Coupon code" value={form.code} onChangeText={code => setForm({ ...form, code })} autoCapitalize="characters" maxLength={24} placeholder="FALL10" style={ui.input} />
      <View style={styles.choices}>
        <Pressable accessibilityRole="button" onPress={() => setForm({ ...form, kind: "percent" })} style={[styles.choice, form.kind === "percent" && styles.choiceOn]}>
          <Text style={[styles.choiceText, form.kind === "percent" && styles.choiceTextOn]}>Percent</Text>
        </Pressable>
        <Pressable accessibilityRole="button" onPress={() => setForm({ ...form, kind: "fixed" })} style={[styles.choice, form.kind === "fixed" && styles.choiceOn]}>
          <Text style={[styles.choiceText, form.kind === "fixed" && styles.choiceTextOn]}>Fixed</Text>
        </Pressable>
      </View>
      <TextInput accessibilityLabel={form.kind === "percent" ? "Basis points" : "Amount"} value={form.amount} onChangeText={amount => setForm({ ...form, amount })} keyboardType="number-pad" placeholder={form.kind === "percent" ? "1000" : "500"} style={ui.input} />
      <TextInput accessibilityLabel="Currency" value={form.currency} onChangeText={currency => setForm({ ...form, currency })} autoCapitalize="characters" maxLength={3} style={ui.input} />
      <TextInput accessibilityLabel="Starts" value={form.startsOn} onChangeText={startsOn => setForm({ ...form, startsOn })} placeholder="YYYY-MM-DD" style={ui.input} />
      <TextInput accessibilityLabel="Ends" value={form.endsOn} onChangeText={endsOn => setForm({ ...form, endsOn })} placeholder="YYYY-MM-DD" style={ui.input} />
      {needsUnlock ? <TextInput accessibilityLabel="Wallet passphrase" value={passphrase} onChangeText={setPassphrase} secureTextEntry autoCapitalize="none" style={ui.input} /> : null}
      {error ? <Notice tone="error" text={error} /> : null}
      <Button label={busy ? "Signing…" : "Sign coupon"} busy={busy} onPress={() => void save()} />
      {coupons.map(coupon => (
        <Text key={coupon.code} style={ui.muted}>{coupon.code} · {coupon.percentBps != null ? `${coupon.percentBps} bps` : coupon.fixed} {coupon.currency}</Text>
      ))}
    </Card>
  );
}

function BulkEdit({ listings, onDone }: { listings: ListingHeader[]; onDone: () => void }) {
  const choices = listings.filter(listing => listing.listingId);
  const [selected, setSelected] = useState<string[]>([]);
  const [bulk, setBulk] = useState<"price" | "availability">("price");
  const [price, setPrice] = useState("");
  const [availability, setAvailability] = useState<AvailabilityDraft>({ kind: "in-stock", shipsOn: "", days: "" });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [needsUnlock, setNeedsUnlock] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  if (!choices.length) return null;
  async function apply() {
    const ids = selected.filter(id => choices.some(listing => listing.listingId === id));
    if (!ids.length) return;
    let change: Parameters<typeof bulkEditListings>[1];
    if (bulk === "price") {
      if (!/^\d+$/.test(price.trim()) || Number(price) === 0) {
        setError(price.trim() === "0" ? "Enter a price above zero." : "The price must be a whole number of the smallest unit.");
        return;
      }
      change = { price: Number(price) };
    } else {
      const terms = purchaseTerms("1", availability);
      if (typeof terms === "string") {
        setError(terms);
        return;
      }
      change = { availability: terms.availability };
    }
    setBusy(true);
    setError(null);
    try {
      if (needsUnlock) {
        await unlockWallet(passphrase);
        setPassphrase("");
        setNeedsUnlock(false);
      }
      await bulkEditListings(ids, change);
      setSelected([]);
      onDone();
    } catch (err) {
      if (isLocked(err)) {
        setNeedsUnlock(true);
        setError("Unlock the wallet on your node so it can sign these listings.");
      } else setError(passphraseError(err, "Those listings were not changed."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Card title="Edit selected">
      <Text style={ui.muted}>One price or one availability. A draft stays a draft.</Text>
      <View style={styles.choices}>
        <Pressable accessibilityRole="button" onPress={() => setBulk("price")} style={[styles.choice, bulk === "price" && styles.choiceOn]}>
          <Text style={[styles.choiceText, bulk === "price" && styles.choiceTextOn]}>Price</Text>
        </Pressable>
        <Pressable accessibilityRole="button" onPress={() => setBulk("availability")} style={[styles.choice, bulk === "availability" && styles.choiceOn]}>
          <Text style={[styles.choiceText, bulk === "availability" && styles.choiceTextOn]}>Availability</Text>
        </Pressable>
      </View>
      {bulk === "price" ? (
        <TextInput accessibilityLabel="Regular price" value={price} onChangeText={setPrice} keyboardType="number-pad" placeholder="1800" style={ui.input} />
      ) : (
        <View style={styles.choices}>
          {(["in-stock", "pre-order", "made-to-order", "away"] as const).map(kind => (
            <Pressable key={kind} accessibilityRole="button" onPress={() => setAvailability({ ...availability, kind })} style={[styles.choice, availability.kind === kind && styles.choiceOn]}>
              <Text style={[styles.choiceText, availability.kind === kind && styles.choiceTextOn]}>{kind}</Text>
            </Pressable>
          ))}
        </View>
      )}
      {availability.kind === "pre-order" && bulk === "availability" ? <TextInput accessibilityLabel="Ships on" value={availability.shipsOn} onChangeText={shipsOn => setAvailability({ ...availability, shipsOn })} placeholder="YYYY-MM-DD" style={ui.input} /> : null}
      {availability.kind === "made-to-order" && bulk === "availability" ? <TextInput accessibilityLabel="Ready in days" value={availability.days} onChangeText={days => setAvailability({ ...availability, days })} keyboardType="number-pad" placeholder="14" style={ui.input} /> : null}
      {choices.map(listing => (
        <Pressable key={listing.listingId} accessibilityRole="checkbox" accessibilityState={{ checked: selected.includes(listing.listingId!) }} onPress={() => setSelected(current => current.includes(listing.listingId!) ? current.filter(id => id !== listing.listingId) : [...current, listing.listingId!])}>
          <Text style={ui.muted}>{selected.includes(listing.listingId!) ? "Selected" : "Select"} {listing.title}</Text>
        </Pressable>
      ))}
      {needsUnlock ? <TextInput accessibilityLabel="Wallet passphrase" value={passphrase} onChangeText={setPassphrase} secureTextEntry autoCapitalize="none" style={ui.input} /> : null}
      {error ? <Notice tone="error" text={error} /> : null}
      <Button label={busy ? "Signing…" : "Apply to selected"} busy={busy} disabled={!selected.length} onPress={() => void apply()} />
    </Card>
  );
}

function DisplayName({ saved }: { saved: string }) {
  const [name, setName] = useState(saved);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [needsUnlock, setNeedsUnlock] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const refused = displayNameError(name);
  async function save() {
    if (refused) {
      setError(refused);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (needsUnlock) {
        await unlockWallet(passphrase);
        setPassphrase("");
        setNeedsUnlock(false);
      }
      const next = await updateProfile({ name: name.trim() });
      setName(next.name);
      await sync();
    } catch (err) {
      if (isLocked(err)) {
        setNeedsUnlock(true);
        setError("Unlock the wallet on your node so it can sign this name.");
      } else setError(passphraseError(err, "The name was not saved."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Card title="Display name">
      <Text style={ui.muted}>Buyers see this name on your listings. Saved name: {saved}</Text>
      <TextInput accessibilityLabel="Display name" value={name} onChangeText={setName} maxLength={64} autoCapitalize="words" style={ui.input} />
      {refused ? <Notice tone="error" text={refused} /> : null}
      {needsUnlock ? <TextInput accessibilityLabel="Wallet passphrase" value={passphrase} onChangeText={setPassphrase} secureTextEntry autoCapitalize="none" style={ui.input} /> : null}
      {error && error !== refused ? <Notice tone="error" text={error} /> : null}
      <Button label={busy ? "Signing…" : "Save name"} busy={busy} disabled={refused !== null} onPress={() => void save()} />
    </Card>
  );
}

export function StoreScreen({ onRefresh }: { onRefresh: () => void }) {
  const session = useSession();
  const { catalog, error } = useCatalog();
  const [editing, setEditing] = useState<ListingHeader | SavedDraft | null | "new">(null);
  const [drafts, setDrafts] = useState<SavedDraft[]>([]);
  const queuedTargets = new Set([...session.queue.pending, ...(session.queue.inFlight?.requests ?? [])].map(m => (m.body as { target?: string } | undefined)?.target));
  const collection = session.replica.collections.catalog;
  const own = catalog?.stores.find((store) => store.own);
  useEffect(() => {
    if (!own || editing !== null) return;
    void fetchDrafts().then(setDrafts).catch(() => setDrafts([]));
  }, [own, editing]);
  const categories = [...new Set(catalog?.listings.map((listing) => listing.category) ?? [])];
  if (editing !== null) {
    const unpublished = editing !== "new" && "draftId" in editing ? editing : null;
    const published = editing !== "new" && !("draftId" in editing) ? editing : null;
    return <ListingEdit listing={published} unpublished={unpublished} categories={categories} onClose={() => setEditing(null)} />;
  }
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>{own?.name || catalog?.store.name || "Your store"}</Text>
      {session.lastError ? <Notice tone="error" text={session.lastError} /> : null}
      {error ? <Notice tone="error" text={error} /> : null}
      {session.refused.length ? (
        <Card title="Changes the node refused">
          {session.refused.map((outcome, i) => (
            <Text key={i} style={ui.muted}>
              {outcome.mutation.label}: {String((outcome.result.body as { error?: string } | null)?.error ?? outcome.result.status)}
            </Text>
          ))}
          <Button label="Dismiss" kind="plain" onPress={dismissRefused} />
        </Card>
      ) : null}
      {!catalog && !error ? <Notice text={session.syncing ? "Loading your store from your node…" : "Your store has not synced to this phone yet."} /> : null}
      {catalog ? (
        <>
          {own ? <DisplayName key={own.key} saved={own.name} /> : null}
          <Card title="Overview">
            <Row label="Stores" value={String(catalog.stores.length)} />
            <Row label="Listings" value={String(catalog.listings.length)} />
            <Row label="Rating" value={catalog.store.contentRating} />
            {catalog.rejected ? <Row label="Failed signature check" value={String(catalog.rejected)} /> : null}
            {collection?.error ? <Row label="Last sync" value={`failed (${collection.error.code})`} /> : null}
          </Card>
          {drafts.length ? (
            <Card title="Drafts">
              <Text style={ui.muted}>Saved on this node. Buyers do not see them until you publish.</Text>
              {drafts.map((draft) => (
                <Button key={draft.draftId} label={draft.listing.title || "Untitled draft"} kind="plain" onPress={() => setEditing(draft)} />
              ))}
            </Card>
          ) : null}
          {own ? <CouponEditor onDone={onRefresh} /> : null}
          <BulkEdit listings={catalog.listings.filter(listing => own && listing.storeId === own.key)} onDone={onRefresh} />
          <Card title="Listings">
            {own ? <Button label="New listing" onPress={() => setEditing("new")} /> : <Text style={ui.muted}>This node has no storefront yet. Set the store up in the web shop, then you can add listings here.</Text>}
            {catalog.listings.length === 0 ? <Text style={ui.muted}>No listings yet.</Text> : null}
            {catalog.listings.map(listing => (
              <Listing
                key={listing.hash}
                listing={listing}
                queued={queuedTargets.has(listing.hash)}
                canEdit={!!own && listing.storeId === own.key && !!listing.listingId}
                ownWithoutId={!!own && listing.storeId === own.key && !listing.listingId}
                onEdit={() => setEditing(listing)}
                onRate={rating =>
                  void queueEdit({
                    method: "POST",
                    path: "/content-labels",
                    body: { target: listing.hash, rating, tags: listing.contentTags },
                    label: `Rate “${listing.title}” ${rating}`,
                  })
                }
              />
            ))}
          </Card>
        </>
      ) : null}
      <Button label={session.syncing ? "Syncing…" : "Sync now"} kind="plain" busy={session.syncing} onPress={onRefresh} />
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  listing: { borderTopWidth: 1, borderTopColor: LINE, paddingTop: 16 },
  listingHead: { flexDirection: "row", alignItems: "center", gap: 10, minHeight: 44 },
  thumb: { width: 56, height: 56, borderRadius: 8, backgroundColor: SOFT },
  listingText: { flex: 1, gap: 6 },
  listingTitle: { fontSize: 16, fontWeight: "600", color: NAVY },
  chevron: { fontSize: 22, color: MUTED, width: 22, textAlign: "center" },
  rating: { gap: 8, paddingTop: 10 },
  choices: { flexDirection: "row", flexWrap: "wrap", gap: 10 },
  choice: { backgroundColor: SOFT, borderRadius: 999, minHeight: 44, justifyContent: "center", paddingVertical: 10, paddingHorizontal: 14 },
  choiceOn: { backgroundColor: NAVY },
  choiceText: { color: NAVY, fontWeight: "600" },
  choiceTextOn: { color: "#fff" },
});
