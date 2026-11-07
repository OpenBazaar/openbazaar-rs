import { useMemo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import { parseCatalog, type ContentRating, type ListingHeader, type ShopCatalog } from "../../catalog.ts";
import { collectionValue } from "../../homeNode/replica.ts";
import { ListingPhoto } from "../ListingPhoto.tsx";
import { dismissRefused, queueEdit, useSession } from "../session.ts";
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

export function StoreScreen({ onRefresh }: { onRefresh: () => void }) {
  const session = useSession();
  const { catalog, error } = useCatalog();
  const [editing, setEditing] = useState<ListingHeader | null | "new">(null);
  const queuedTargets = new Set([...session.queue.pending, ...(session.queue.inFlight?.requests ?? [])].map(m => (m.body as { target?: string } | undefined)?.target));
  const collection = session.replica.collections.catalog;
  const own = catalog?.stores.find((store) => store.own);
  const categories = [...new Set(catalog?.listings.map((listing) => listing.category) ?? [])];
  if (editing !== null) {
    return <ListingEdit listing={editing === "new" ? null : editing} categories={categories} onClose={() => setEditing(null)} />;
  }
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>{catalog?.store.name || "Your store"}</Text>
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
          <Card title="Overview">
            <Row label="Stores" value={String(catalog.stores.length)} />
            <Row label="Listings" value={String(catalog.listings.length)} />
            <Row label="Rating" value={catalog.store.contentRating} />
            {catalog.rejected ? <Row label="Failed signature check" value={String(catalog.rejected)} /> : null}
            {collection?.error ? <Row label="Last sync" value={`failed (${collection.error.code})`} /> : null}
          </Card>
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
  listing: { borderTopWidth: 1, borderTopColor: LINE, paddingTop: 10 },
  listingHead: { flexDirection: "row", alignItems: "center", gap: 10, minHeight: 44 },
  thumb: { width: 56, height: 56, borderRadius: 8, backgroundColor: SOFT },
  listingText: { flex: 1, gap: 2 },
  listingTitle: { fontSize: 16, fontWeight: "600", color: NAVY },
  chevron: { fontSize: 22, color: MUTED, width: 22, textAlign: "center" },
  rating: { gap: 8, paddingTop: 10 },
  choices: { flexDirection: "row", gap: 8 },
  choice: { backgroundColor: SOFT, borderRadius: 999, paddingVertical: 8, paddingHorizontal: 14 },
  choiceOn: { backgroundColor: NAVY },
  choiceText: { color: NAVY, fontWeight: "600" },
  choiceTextOn: { color: "#fff" },
});
