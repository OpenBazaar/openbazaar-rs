import { useState } from "react";
import { Alert, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { File } from "expo-file-system";

import type { ListingHeader } from "../../catalog.ts";
import { TAXONOMY, compareAtText, listingFacts, packageFieldsOf, priceTerms, purchaseFields, purchaseTerms, returnsDraft, saleFieldsOf, scheduledSale, shippingTerms, signedTaxonomy, specificRows, tierRows, volumeTiers, withFacts, withPackage, withPrice, withPurchase, withSale, withTaxonomy, withTiers, withVariantWeights, type AvailabilityDraft, type PackageInput, type ReturnsDraft, type SpecificRow, type TierRow } from "../../commerceDraft.ts";
import { CONTENT_TAGS, MAX_LISTING_IMAGES, PRICE_CURRENCIES, deleteListing, draftOf, fromMinorUnits, saveListing, toMinorUnits, uploadImage, type ListingDraft, type PriceCurrency } from "../../seller.ts";
import { isLocked, passphraseError, unlockWallet } from "../../walletLock.ts";
import { sync } from "../session.ts";
import { ListingPhoto } from "../ListingPhoto.tsx";
import { Button, Card, NAVY, Notice, SOFT, ui } from "../ui.tsx";

type PackageFields = Omit<PackageInput, "regions">;
type Fields = Omit<ListingDraft, "price" | "shipping" | "stock"> & PackageFields & { price: string; shipping: string; stock: string; salePrice: string; saleStarts: string; saleEnds: string; compareAt: string; acceptsOffers: boolean; lotSize: string; availability: AvailabilityDraft; tiers: TierRow[]; variantWeights: string[]; taxonomy: string; shipsTo: string[]; specifics: SpecificRow[]; returns: ReturnsDraft };

const EMPTY: ListingDraft = {
  title: "",
  description: "",
  category: "",
  kind: "physical",
  shipFrom: "",
  stock: 1,
  images: [],
  currency: "USD",
  price: 0,
  shipping: 0,
  contentRating: "general",
  contentTags: [],
  commerce: null,
};

function fieldsOf(listing: ListingHeader | null): Fields {
  const draft = listing ? draftOf(listing) : EMPTY;
  const sale = saleFieldsOf(draft.price, draft.commerce?.sale ?? null, draft.currency);
  return {
    ...draft,
    ...packageFieldsOf(draft.commerce?.shipping, draft.currency),
    price: draft.price ? fromMinorUnits(draft.price, draft.currency) : "",
    shipping: draft.shipping ? fromMinorUnits(draft.shipping, draft.currency) : "",
    stock: String(draft.stock),
    salePrice: sale.price,
    saleStarts: sale.startsOn,
    saleEnds: sale.endsOn,
    compareAt: compareAtText(draft.commerce?.compareAt ?? 0, draft.currency),
    acceptsOffers: draft.commerce?.acceptsOffers ?? false,
    lotSize: purchaseFields(draft.commerce).lotSize,
    availability: purchaseFields(draft.commerce).availability,
    tiers: tierRows(draft.price, draft.commerce?.tiers ?? [], draft.currency),
    variantWeights: draft.commerce?.variants.map(variant => (variant.weightG ? String(variant.weightG) : "")) ?? [],
    taxonomy: String(draft.commerce?.taxonomy ?? 0),
    shipsTo: draft.commerce?.shipping.services.map(service => service.regions.join(", ")) ?? [],
    specifics: specificRows(draft.commerce?.specifics),
    returns: returnsDraft(draft.commerce?.returns),
  };
}

function draftFrom(form: Fields): ListingDraft | string {
  const price = toMinorUnits(form.price, form.currency);
  const shipping = form.kind === "digital" || !form.shipping.trim() ? 0 : toMinorUnits(form.shipping, form.currency);
  const stock = Number(form.stock);
  if (!form.title.trim()) return "Give the listing a title.";
  if (!form.category.trim()) return "Choose a category.";
  if (!price) return form.currency === "SAT" ? "Enter a price in whole sats." : `Enter a price in ${form.currency}, like 12.50.`;
  if (shipping === null) return "Enter shipping as a plain amount, or leave it empty for free shipping.";
  if (!Number.isInteger(stock) || stock < 0 || stock > 1_000_000) return "Enter how many you have, from 0 to 1,000,000.";
  if (form.kind === "physical" && !form.shipFrom.trim()) return "Say where it ships from.";
  const sale = scheduledSale(price, form.currency, { price: form.salePrice, startsOn: form.saleStarts, endsOn: form.saleEnds });
  if (typeof sale === "string") return sale;
  let commerce = withSale(form.commerce ?? null, sale);
  if (form.kind === "physical") {
    const terms = shippingTerms(form.currency, {
      weightG: form.weightG, lengthMm: form.lengthMm, widthMm: form.widthMm, heightMm: form.heightMm,
      handlingDays: form.handlingDays, freeOver: form.freeOver,
      regions: form.shipsTo,
    });
    if (typeof terms === "string") return terms;
    commerce = withPackage(commerce, terms);
  }
  const facts = listingFacts(form.specifics, form.returns);
  if (typeof facts === "string") return facts;
  commerce = withFacts(commerce, facts);
  const priced = priceTerms(price, form.currency, form.compareAt, form.acceptsOffers, (commerce ?? form.commerce)?.variants.map(variant => variant.price) ?? []);
  if (typeof priced === "string") return priced;
  commerce = withPrice(commerce, priced);
  const purchase = purchaseTerms(form.lotSize, form.availability);
  if (typeof purchase === "string") return purchase;
  commerce = withPurchase(commerce, purchase);
  const tiers = volumeTiers(price, form.currency, form.tiers);
  if (typeof tiers === "string") return tiers;
  commerce = withTiers(commerce, tiers);
  if (commerce?.variants.length) {
    const variants = withVariantWeights(commerce.variants, form.variantWeights);
    if (typeof variants === "string") return variants;
    commerce = { ...commerce, variants };
  }
  const taxonomy = signedTaxonomy(form.taxonomy);
  if (typeof taxonomy === "string") return taxonomy;
  commerce = withTaxonomy(commerce, taxonomy);
  if (commerce && form.currency === "SAT") return "Price a listing with options or shipping services in USD, EUR, GBP, or JPY.";
  return {
    title: form.title.trim(),
    description: form.description.trim(),
    category: form.category.trim(),
    kind: form.kind,
    shipFrom: form.shipFrom.trim(),
    stock,
    images: form.images,
    currency: form.currency,
    price,
    shipping,
    contentRating: form.contentRating,
    contentTags: form.contentTags,
    commerce,
  };
}

export function ListingEdit({ listing, categories, onClose }: { listing: ListingHeader | null; categories: string[]; onClose: () => void }) {
  const [form, setForm] = useState<Fields>(() => fieldsOf(listing));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [needsUnlock, setNeedsUnlock] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const variants = (form.commerce?.variants.length ?? 0) > 0;
  const services = (form.commerce?.shipping.services.length ?? 0) > 0;
  const set = <K extends keyof Fields>(key: K, value: Fields[K]) => setForm((current) => ({ ...current, [key]: value }));

  const persist = async (draft: ListingDraft) => {
    await saveListing(listing?.listingId ?? null, draft);
    await sync();
    onClose();
  };

  const save = async () => {
    const draft = draftFrom(form);
    if (typeof draft === "string") {
      setError(draft);
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
      await persist(draft);
    } catch (err) {
      if (isLocked(err)) {
        setNeedsUnlock(true);
        setError("Unlock the wallet on your node so it can sign this listing.");
      } else {
        setError(passphraseError(err, "The listing was not saved."));
      }
    } finally {
      setBusy(false);
    }
  };

  const addPhoto = async () => {
    if (form.images.length >= MAX_LISTING_IMAGES) return;
    setError(null);
    try {
      const picked = await File.pickFileAsync({ mimeTypes: ["image/png", "image/jpeg", "image/webp", "image/gif"] });
      if (picked.canceled || !picked.result) return;
      setBusy(true);
      const hash = await uploadImage(picked.result);
      setForm((current) => (current.images.includes(hash) || current.images.length >= MAX_LISTING_IMAGES ? current : { ...current, images: [...current.images, hash] }));
    } catch (err) {
      setError(err instanceof Error ? err.message : "The photo was not uploaded.");
    } finally {
      setBusy(false);
    }
  };

  const remove = () => {
    if (!listing?.listingId) return;
    Alert.alert(`Delete “${listing.title}”?`, "It leaves your store. Buyers will no longer see it.", [
      { text: "Cancel", style: "cancel" },
      {
        text: "Delete",
        style: "destructive",
        onPress: () => {
          setBusy(true);
          setError(null);
          void (async () => {
            if (needsUnlock) {
              await unlockWallet(passphrase);
              setPassphrase("");
              setNeedsUnlock(false);
            }
            await deleteListing(listing.listingId as string);
            await sync();
            onClose();
          })()
            .catch((err: unknown) => {
              if (isLocked(err)) {
                setNeedsUnlock(true);
                setError("Unlock the wallet on your node so it can sign this change.");
              } else setError(passphraseError(err, "The listing was not deleted."));
            })
            .finally(() => setBusy(false));
        },
      },
    ]);
  };

  const unit = form.currency === "SAT" ? "sats" : form.currency;
  return (
    <ScrollView contentContainerStyle={ui.screen} keyboardShouldPersistTaps="handled">
      <Text style={ui.title}>{listing ? "Edit listing" : "New listing"}</Text>
      {error ? <Notice tone="error" text={error} /> : null}
      <Card title="Photos">
        <View style={styles.photos}>
          {form.images.map((hash, index) => (
            <View key={hash} style={styles.photo}>
              <ListingPhoto hash={hash} title={form.title || "Listing photo"} style={styles.photoImage} />
              <Pressable accessibilityRole="button" accessibilityLabel={`Remove photo ${index + 1}`} onPress={() => set("images", form.images.filter((item) => item !== hash))} style={styles.removePhoto}>
                <Text style={styles.removePhotoText}>Remove</Text>
              </Pressable>
            </View>
          ))}
        </View>
        {form.images.length < MAX_LISTING_IMAGES ? <Button label="Add a photo" kind="plain" disabled={busy} onPress={() => void addPhoto()} /> : null}
      </Card>
      <Card title="Details">
        <Field label="Title" value={form.title} onChangeText={(title) => set("title", title)} />
        <Field label="Description" value={form.description} onChangeText={(description) => set("description", description)} multiline />
        <Field label="Category" value={form.category} onChangeText={(category) => set("category", category)} />
        {form.currency !== "SAT" ? (
          <>
            <Text style={ui.muted}>Top-level category</Text>
            <View style={styles.choices}>
              <Chip label="Uncategorised" selected={form.taxonomy === "0"} onPress={() => set("taxonomy", "0")} />
              {TAXONOMY.map((item) => (
                <Chip key={item.id} label={item.name} selected={form.taxonomy === String(item.id)} onPress={() => set("taxonomy", String(item.id))} />
              ))}
            </View>
            <Text style={ui.muted}>Stored on the listing. The category above stays as the subcategory.</Text>
          </>
        ) : null}
        {categories.length ? (
          <View style={styles.choices}>
            {categories.map((category) => (
              <Chip key={category} label={category} selected={form.category === category} onPress={() => set("category", category)} />
            ))}
          </View>
        ) : null}
        <Text style={ui.muted}>Type of good</Text>
        <View style={styles.choices}>
          <Chip label="Physical item" selected={form.kind === "physical"} onPress={() => set("kind", "physical")} />
          <Chip label="Digital good" selected={form.kind === "digital"} onPress={() => set("kind", "digital")} />
        </View>
      </Card>
      <Card title="Price and stock">
        <Text style={ui.muted}>Price currency</Text>
        <View style={styles.choices}>
          {PRICE_CURRENCIES.map((currency) => (
            <Chip key={currency} label={currency === "SAT" ? "sats" : currency} selected={form.currency === currency} onPress={() => set("currency", currency as PriceCurrency)} />
          ))}
        </View>
        <Field label={`Price (${unit})`} value={form.price} onChangeText={(price) => set("price", price)} keyboard="decimal-pad" />
        {form.currency !== "SAT" ? (
          <>
            <Field label={`Sale price (${unit})`} value={form.salePrice} onChangeText={(salePrice) => set("salePrice", salePrice)} keyboard="decimal-pad" />
            <Field label="Sale starts" value={form.saleStarts} onChangeText={(saleStarts) => set("saleStarts", saleStarts)} />
            <Field label="Sale ends" value={form.saleEnds} onChangeText={(saleEnds) => set("saleEnds", saleEnds)} />
            <Field label={`Compare-at (${unit})`} value={form.compareAt} onChangeText={(compareAt) => set("compareAt", compareAt)} keyboard="decimal-pad" />
            <Chip label="Accept offers" selected={form.acceptsOffers} onPress={() => set("acceptsOffers", !form.acceptsOffers)} />
            <Field label="Lot size" value={form.lotSize} onChangeText={(lotSize) => set("lotSize", lotSize)} keyboard="number-pad" />
            <Text style={ui.muted}>Availability</Text>
            <View style={styles.choices}>
              {(["in-stock", "pre-order", "made-to-order", "away"] as const).map((kind) => (
                <Chip key={kind} label={kind === "in-stock" ? "In stock" : kind === "pre-order" ? "Pre-order" : kind === "made-to-order" ? "Made to order" : "Away"} selected={form.availability.kind === kind} onPress={() => set("availability", { ...form.availability, kind })} />
              ))}
            </View>
            {form.availability.kind === "pre-order" ? <Field label="Ships on" value={form.availability.shipsOn} onChangeText={(shipsOn) => set("availability", { ...form.availability, shipsOn })} /> : null}
            {form.availability.kind === "made-to-order" ? <Field label="Ready in (days)" value={form.availability.days} onChangeText={(days) => set("availability", { ...form.availability, days })} keyboard="number-pad" /> : null}
            {form.tiers.map((tier, index) => (
              <View key={index} style={styles.field}>
                <Field label={`Buy at least ${index + 1}`} value={tier.qty} onChangeText={(qty) => set("tiers", form.tiers.map((item, at) => (at === index ? { ...item, qty } : item)))} keyboard="number-pad" />
                <Field label={`Each (${unit})`} value={tier.price} onChangeText={(tierPrice) => set("tiers", form.tiers.map((item, at) => (at === index ? { ...item, price: tierPrice } : item)))} keyboard="decimal-pad" />
                {form.tiers.length > 1 ? <Button label="Remove tier" kind="plain" onPress={() => set("tiers", form.tiers.filter((_, at) => at !== index))} /> : null}
              </View>
            ))}
            {form.tiers.length < 4 ? <Button label="Add a volume tier" kind="plain" onPress={() => set("tiers", [...form.tiers, { qty: "", price: "" }])} /> : null}
            {form.variantWeights.map((weight, index) => (
              <Field
                key={form.commerce?.variants[index]?.id || `weight-${index}`}
                label={`Weight (g) · ${form.commerce?.variants[index]?.options.join(" / ") || "variant"}`}
                value={weight}
                onChangeText={(next) => set("variantWeights", form.variantWeights.map((item, at) => (at === index ? next : item)))}
                keyboard="number-pad"
              />
            ))}
            <Text style={ui.muted}>Leave the sale empty to sell at the regular price. Use days like 2026-11-01. Compare-at is above the price buyers pay. A blank tier row is left off the listing, and each tier price is below the regular price. Variant weight is grams; leave it empty to use the package weight. These are signed with the listing.</Text>
          </>
        ) : null}
        {form.kind === "physical" && !services ? <Field label={`Shipping (${unit})`} value={form.shipping} onChangeText={(shipping) => set("shipping", shipping)} keyboard="decimal-pad" /> : null}
        {services ? <Text style={ui.muted}>Service prices and delivery days stay as they are. You can still set where each one ships.</Text> : null}
        {form.kind === "physical" && form.currency !== "SAT" ? (
          <>
            <Field label="Package weight (g)" value={form.weightG} onChangeText={(weightG) => set("weightG", weightG)} keyboard="number-pad" />
            <Field label="Length (mm)" value={form.lengthMm} onChangeText={(lengthMm) => set("lengthMm", lengthMm)} keyboard="number-pad" />
            <Field label="Width (mm)" value={form.widthMm} onChangeText={(widthMm) => set("widthMm", widthMm)} keyboard="number-pad" />
            <Field label="Height (mm)" value={form.heightMm} onChangeText={(heightMm) => set("heightMm", heightMm)} keyboard="number-pad" />
            <Field label="Handling time (days)" value={form.handlingDays} onChangeText={(handlingDays) => set("handlingDays", handlingDays)} keyboard="number-pad" />
            <Field label={`Free shipping over (${unit})`} value={form.freeOver} onChangeText={(freeOver) => set("freeOver", freeOver)} keyboard="decimal-pad" />
            {form.shipsTo.map((line, index) => (
              <Field
                key={form.commerce?.shipping.services[index]?.name ?? `ships-${index}`}
                label={`Ships to (${form.commerce?.shipping.services[index]?.name || "service"})`}
                value={line}
                onChangeText={(text) => set("shipsTo", form.shipsTo.map((item, at) => (at === index ? text : item)))}
              />
            ))}
            <Text style={ui.muted}>Country codes, like US, CA. Leave ships-to empty for worldwide. Handling time is at most 30 days.</Text>
          </>
        ) : null}
        {form.kind === "physical" ? <Field label="Ships from" value={form.shipFrom} onChangeText={(shipFrom) => set("shipFrom", shipFrom)} /> : null}
        {variants ? <Text style={ui.muted}>Stock follows the options already on this listing. This screen does not change those options.</Text> : <Field label="In stock" value={form.stock} onChangeText={(stock) => set("stock", stock)} keyboard="number-pad" />}
      </Card>
      {form.currency !== "SAT" ? (
        <Card title="Specifics and returns">
          {form.specifics.map((row, index) => (
            <View key={index} style={styles.field}>
              <Field label={`Specific ${index + 1}`} value={row.key} onChangeText={(key) => set("specifics", form.specifics.map((item, at) => (at === index ? { ...item, key } : item)))} />
              <Field label="Value" value={row.value} onChangeText={(value) => set("specifics", form.specifics.map((item, at) => (at === index ? { ...item, value } : item)))} />
              {form.specifics.length > 1 ? <Button label="Remove specific" kind="plain" onPress={() => set("specifics", form.specifics.filter((_, at) => at !== index))} /> : null}
            </View>
          ))}
          {form.specifics.length < 24 ? <Button label="Add an item specific" kind="plain" onPress={() => set("specifics", [...form.specifics, { key: "", value: "" }])} /> : null}
          <Text style={ui.muted}>Returns</Text>
          <View style={styles.choices}>
            {(["store", "none", "days"] as const).map((kind) => (
              <Chip key={kind} label={kind === "store" ? "Store policy" : kind === "none" ? "No returns" : "Set days"} selected={form.returns.kind === kind} onPress={() => set("returns", { ...form.returns, kind })} />
            ))}
          </View>
          {form.returns.kind === "days" ? (
            <>
              <Field label="Return window (days)" value={form.returns.days} onChangeText={(days) => set("returns", { ...form.returns, days })} keyboard="number-pad" />
              <Chip label="Seller pays return shipping" selected={form.returns.sellerPays} onPress={() => set("returns", { ...form.returns, sellerPays: !form.returns.sellerPays })} />
            </>
          ) : null}
        </Card>
      ) : null}
      <Card title="Content label">
        <View style={styles.choices}>
          {(["general", "sensitive", "adult"] as const).map((rating) => (
            <Chip key={rating} label={rating} selected={form.contentRating === rating} onPress={() => set("contentRating", rating)} />
          ))}
        </View>
        <View style={styles.choices}>
          {CONTENT_TAGS.map((tag) => (
            <Chip
              key={tag}
              label={tag}
              selected={form.contentTags.includes(tag)}
              onPress={() => set("contentTags", form.contentTags.includes(tag) ? form.contentTags.filter((item) => item !== tag) : [...form.contentTags, tag])}
            />
          ))}
        </View>
      </Card>
      {needsUnlock ? (
        <Card title="Wallet">
          <Field label="Wallet passphrase" value={passphrase} onChangeText={setPassphrase} secure />
          <Text style={ui.muted}>The node signs the listing with your store key. The passphrase stays on this request.</Text>
        </Card>
      ) : null}
      <Button label={busy ? "Saving…" : listing ? "Save changes" : "Publish listing"} busy={busy} onPress={() => void save()} />
      <Button label="Cancel" kind="plain" disabled={busy} onPress={onClose} />
      {listing?.listingId ? <Button label="Delete listing" kind="danger" disabled={busy} onPress={remove} /> : null}
    </ScrollView>
  );
}

function Field({ label, value, onChangeText, keyboard, multiline, secure }: { label: string; value: string; onChangeText: (value: string) => void; keyboard?: "decimal-pad" | "number-pad"; multiline?: boolean; secure?: boolean }) {
  return (
    <View style={styles.field}>
      <Text style={ui.muted}>{label}</Text>
      <TextInput
        value={value}
        onChangeText={onChangeText}
        accessibilityLabel={label}
        keyboardType={keyboard}
        multiline={multiline}
        secureTextEntry={secure}
        autoCapitalize={secure ? "none" : "sentences"}
        autoCorrect={!secure}
        style={[ui.input, multiline && styles.multiline]}
      />
    </View>
  );
}

function Chip({ label, selected, onPress }: { label: string; selected: boolean; onPress: () => void }) {
  return (
    <Pressable accessibilityRole="button" accessibilityState={{ selected }} onPress={onPress} style={[styles.chip, selected && styles.chipOn]}>
      <Text style={[styles.chipText, selected && styles.chipTextOn]}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  photos: { flexDirection: "row", flexWrap: "wrap", gap: 10 },
  photo: { width: 96, gap: 4 },
  photoImage: { width: 96, height: 96, borderRadius: 8, backgroundColor: SOFT },
  removePhoto: { minHeight: 44, alignItems: "center", justifyContent: "center" },
  removePhotoText: { color: NAVY, fontWeight: "600" },
  choices: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
  chip: { minHeight: 44, justifyContent: "center", backgroundColor: SOFT, borderRadius: 999, paddingHorizontal: 14 },
  chipOn: { backgroundColor: NAVY },
  chipText: { color: NAVY, fontWeight: "600" },
  chipTextOn: { color: "#fff" },
  field: { gap: 4 },
  multiline: { minHeight: 96, textAlignVertical: "top" },
});
