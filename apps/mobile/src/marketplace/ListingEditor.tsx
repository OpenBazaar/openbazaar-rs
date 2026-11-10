import { useState, type FormEvent } from "react";
import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import { CONTENT_TAGS, MAX_LISTING_IMAGES, PRICE_CURRENCIES, deleteDraft, draftOf, formatPrice, fromMinorUnits, saveDraft, saveListing, toMinorUnits, type ListingDraft, type PriceCurrency, type SavedDraft } from "../seller";
import { ImageUpload, MediaImage, useLockedAction } from "./SellerKit";
import { axesProblem, compareAtText, emptyCommerce, listingFacts, packageFieldsOf, priceTerms, purchaseFields, purchaseTerms, returnsDraft, rowsOf, saleFieldsOf, scheduledSale, shippingTerms, signedTaxonomy, specificRows, tierRows, volumeTiers, withFacts, withPackage, withPrice, withPurchase, withSale, withTaxonomy, withTiers, withVariantWeights, type AvailabilityDraft, type PackageInput, type ReturnsDraft, type SpecificRow, type TierRow } from "../commerceDraft";
import { CommerceFields } from "./CommerceFields";

type PackageFields = Omit<PackageInput, "regions">;
type Form = Omit<ListingDraft, "price" | "shipping" | "stock"> & PackageFields & { price: string; shipping: string; stock: string; salePrice: string; saleStarts: string; saleEnds: string; compareAt: string; acceptsOffers: boolean; lotSize: string; availability: AvailabilityDraft; tiers: TierRow[]; variantWeights: string[]; taxonomy: string; specifics: SpecificRow[]; returns: ReturnsDraft };

const EMPTY: ListingDraft = { title: "", description: "", category: "", kind: "physical", shipFrom: "", stock: 1, images: [], currency: "USD", price: 0, shipping: 0, contentRating: "general", contentTags: [] };

function formOf(draft: ListingDraft): Form {
  const sale = saleFieldsOf(draft.price, draft.commerce?.sale ?? null, draft.currency);
  const purchase = purchaseFields(draft.commerce);
  const rows = draft.commerce ? rowsOf(draft.commerce) : [];
  return { ...draft, ...packageFieldsOf(draft.commerce?.shipping, draft.currency), specifics: specificRows(draft.commerce?.specifics), returns: returnsDraft(draft.commerce?.returns), price: draft.price ? fromMinorUnits(draft.price, draft.currency) : "", shipping: draft.shipping ? fromMinorUnits(draft.shipping, draft.currency) : "", stock: String(draft.stock), salePrice: sale.price, saleStarts: sale.startsOn, saleEnds: sale.endsOn, compareAt: compareAtText(draft.commerce?.compareAt ?? 0, draft.currency), acceptsOffers: draft.commerce?.acceptsOffers ?? false, lotSize: purchase.lotSize, availability: purchase.availability, tiers: tierRows(draft.price, draft.commerce?.tiers ?? [], draft.currency), variantWeights: rows.map(row => row.weightG ? String(row.weightG) : ""), taxonomy: String(draft.commerce?.taxonomy ?? 0) };
}

/// The draft the node takes, or what to fix first.
function draftFrom(form: Form): ListingDraft | string {
  const price = toMinorUnits(form.price, form.currency);
  const shipping = form.kind === "digital" || !form.shipping.trim() ? 0 : toMinorUnits(form.shipping, form.currency);
  const stock = Number(form.stock);
  if (!form.title.trim()) return "Give the listing a title.";
  if (!form.category.trim()) return "Give the listing a category.";
  if (!price) return form.currency === "SAT" ? "Enter a price in whole sats." : `Enter a price in ${form.currency}, like 12.50.`;
  if (shipping === null) return "Enter shipping as a plain amount, or leave it empty for free shipping.";
  if (!Number.isInteger(stock) || stock < 0 || stock > 1_000_000) return "Enter how many you have, from 0 to 1,000,000.";
  if (form.kind === "physical" && !form.shipFrom.trim()) return "Say where it ships from.";
  const sale = scheduledSale(price, form.currency, { price: form.salePrice, startsOn: form.saleStarts, endsOn: form.saleEnds });
  if (typeof sale === "string") return sale;
  let commerce = withSale(form.commerce ?? null, sale);
  if (commerce && form.kind === "physical") {
    const terms = shippingTerms(form.currency, {
      weightG: form.weightG, lengthMm: form.lengthMm, widthMm: form.widthMm, heightMm: form.heightMm,
      handlingDays: form.handlingDays, freeOver: form.freeOver,
      regions: commerce.shipping.services.map(service => service.regions.join(", ")),
    });
    if (typeof terms === "string") return terms;
    commerce = withPackage(commerce, terms);
  }
  if (form.commerce) {
    const facts = listingFacts(form.specifics, form.returns);
    if (typeof facts === "string") return facts;
    commerce = withFacts(commerce, facts);
  }
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
    const rows = rowsOf(commerce);
    const weights = form.variantWeights.length === rows.length ? rows.flatMap((row, index) => (row.offered ? [form.variantWeights[index] ?? ""] : [])) : form.variantWeights;
    const variants = withVariantWeights(commerce.variants, weights);
    if (typeof variants === "string") return variants;
    commerce = { ...commerce, variants };
  }
  const taxonomy = signedTaxonomy(form.taxonomy);
  if (typeof taxonomy === "string") return taxonomy;
  commerce = withTaxonomy(commerce, taxonomy);
  if (commerce && form.currency === "SAT") return "Price a listing with options or shipping services in USD, EUR, GBP, or JPY.";
  if (commerce) {
    const problem = axesProblem(commerce.axes, rowsOf(commerce));
    if (problem) return problem;
    if (commerce.shipping.services.some(s => !s.name.trim() || s.daysMin > s.daysMax)) return "Name each shipping service and give its delivery days, fastest first.";
  }
  const { salePrice: _salePrice, saleStarts: _saleStarts, saleEnds: _saleEnds, compareAt: _compareAt, acceptsOffers: _acceptsOffers, lotSize: _lotSize, availability: _availability, tiers: _tiers, variantWeights: _variantWeights, taxonomy: _taxonomy, weightG: _weightG, lengthMm: _lengthMm, widthMm: _widthMm, heightMm: _heightMm, handlingDays: _handlingDays, freeOver: _freeOver, specifics: _specifics, returns: _returns, ...rest } = form;
  return { ...rest, price, shipping, stock, commerce };
}

/// What the price is worth in sats at today's rate, for the seller's reference.
function satsHint(amount: number | null, currency: PriceCurrency, rates: RateTable) {
  if (!amount || currency === "SAT") return null;
  const rate = rates[currency as CurrencyCode];
  if (!rate) return null;
  const major = currency === "JPY" ? amount : amount / 100;
  return `≈ ${Math.round((major / rate) * 1e8).toLocaleString("en-US")} sats today`;
}

export function ListingEditor({ listing, savedDraft, rates, onSaved, onCancel }: { listing: ListingHeader | null; savedDraft?: SavedDraft | null; rates: RateTable; onSaved: (message: string) => void; onCancel: () => void }) {
  const [form, setForm] = useState<Form>(() => formOf(savedDraft?.listing ?? (listing ? draftOf(listing) : EMPTY)));
  const [draftId, setDraftId] = useState(savedDraft?.draftId ?? null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const set = <K extends keyof Form>(key: K, value: Form[K]) => setForm(f => ({ ...f, [key]: value }));
  async function save(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    const publish = (e.nativeEvent as SubmitEvent).submitter?.getAttribute("data-action") !== "draft";
    const draft = draftFrom(form);
    if (typeof draft === "string") {
      setError(draft);
      return;
    }
    setBusy(true);
    setError("");
    await run(async () => {
      if (!publish) {
        const saved = await saveDraft(draftId, draft);
        setDraftId(saved.draftId);
        onSaved("Draft saved");
        return;
      }
      await saveListing(listing?.listingId ?? null, draft);
      if (draftId) await deleteDraft(draftId);
      onSaved(listing ? "Listing updated and signed" : "Listing published");
    });
    setBusy(false);
  }
  const unit = form.currency === "SAT" ? "sats" : form.currency;
  const variants = (form.commerce?.variants.length ?? 0) > 0;
  const services = (form.commerce?.shipping.services.length ?? 0) > 0;
  function configure(section = "ob-listing-options") {
    setForm(current => current.commerce ? current : { ...current, commerce: emptyCommerce(), ...packageFieldsOf(emptyCommerce().shipping, current.currency) });
    requestAnimationFrame(() => document.getElementById(section)?.scrollIntoView({ block: "start" }));
  }
  return <form id="ob-listing-form" className="ob-listing-editor" onSubmit={e => void save(e)} aria-label={listing ? `Edit ${listing.title}` : "New listing"}>
    <nav className="ob-editor-sections" aria-label="Listing sections">
      <a href="#ob-listing-details">Details</a>
      <a href="#ob-listing-pricing">Pricing & inventory</a>
      <a href="#ob-listing-photos">Media</a>
      {form.commerce ? <a href="#ob-listing-options">Variants & specifications</a> : <button type="button" disabled={form.currency === "SAT"} onClick={() => configure()}>Variants & specifications</button>}
      {form.kind === "physical" && (form.commerce ? <a href="#ob-listing-shipping">Shipping</a> : <button type="button" disabled={form.currency === "SAT"} onClick={() => configure("ob-listing-shipping")}>Shipping</button>)}
      {form.commerce ? <a href="#ob-listing-returns">Returns</a> : <button type="button" disabled={form.currency === "SAT"} onClick={() => configure("ob-listing-returns")}>Returns</button>}
      <a href="#ob-listing-content">Content rating</a>
    </nav>
    <fieldset className="ob-store-section" id="ob-listing-photos">
      <legend>Product media</legend>
      <div className="ob-listing-photos">
        {form.images.map((hash, i) => <figure key={hash} className="ob-listing-photo">
          <MediaImage hash={hash} alt={`Photo ${i + 1}`}/>
          <button type="button" className="ob-icon-button" aria-label={`Remove photo ${i + 1}`} onClick={() => set("images", form.images.filter(h => h !== hash))}>×</button>
        </figure>)}
        {form.images.length < MAX_LISTING_IMAGES && <ImageUpload label="Add photos" multiple onUploaded={hash => setForm(f => f.images.includes(hash) || f.images.length >= MAX_LISTING_IMAGES ? f : { ...f, images: [...f.images, hash] })}/>}
      </div>
    </fieldset>
    <fieldset className="ob-store-section" id="ob-listing-details">
      <legend>Listing details</legend>
      <div className="ob-field-grid">
        <label className="ob-field ob-field-wide"><span>Title</span><input value={form.title} maxLength={120} onChange={e => set("title", e.target.value)}/></label>
        <label className="ob-field ob-field-wide"><span>Description</span><textarea value={form.description} maxLength={5000} onChange={e => set("description", e.target.value)}/></label>
        <label className="ob-field"><span>Category</span><input value={form.category} maxLength={40} placeholder="Wool blankets" autoCapitalize="off" onChange={e => set("category", e.target.value)}/></label>
        <div className="ob-field" role="radiogroup" aria-label="Type of good"><span>Type of good</span><div className="ob-kind-choice">
          {(["physical", "digital"] as const).map(kind => <label key={kind} className={form.kind === kind ? "is-selected" : ""}><input type="radio" name="kind" checked={form.kind === kind} onChange={() => set("kind", kind)}/>{kind === "physical" ? "Physical item" : "Digital good"}</label>)}
        </div></div>
      </div>
    </fieldset>
    <fieldset className="ob-store-section" id="ob-listing-pricing">
      <legend>Pricing and inventory</legend>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Price currency</span><select value={form.currency} onChange={e => set("currency", e.target.value as PriceCurrency)}>{PRICE_CURRENCIES.map(c => <option key={c} value={c}>{c === "SAT" ? "Bitcoin (sats)" : c}</option>)}</select></label>
        <label className="ob-field"><span>Price ({unit})</span><input inputMode="decimal" value={form.price} placeholder={form.currency === "SAT" ? "25000" : "12.50"} onChange={e => set("price", e.target.value)}/><small className="ob-field-hint">{satsHint(toMinorUnits(form.price, form.currency), form.currency, rates) ?? (form.currency === "SAT" ? "Buyers pay exactly this many sats." : "Buyers pay the sats this is worth when they order.")}</small></label>
        {form.kind === "physical" && !services && <label className="ob-field"><span>Shipping ({unit})</span><input inputMode="decimal" value={form.shipping} placeholder="Free" onChange={e => set("shipping", e.target.value)}/></label>}
        {form.kind === "physical" && <label className="ob-field"><span>Ships from</span><input value={form.shipFrom} maxLength={40} placeholder="Portland" autoCapitalize="off" onChange={e => set("shipFrom", e.target.value)}/></label>}
        {!variants && <label className="ob-field"><span>In stock</span><input inputMode="numeric" value={form.stock} onChange={e => set("stock", e.target.value)}/></label>}
        {form.currency !== "SAT" && <>
          <label className="ob-field"><span>Sale price ({unit})</span><input inputMode="decimal" value={form.salePrice} placeholder="Optional" onChange={e => set("salePrice", e.target.value)}/></label>
          <label className="ob-field"><span>Sale starts</span><input type="date" value={form.saleStarts} onChange={e => set("saleStarts", e.target.value)}/></label>
          <label className="ob-field"><span>Sale ends</span><input type="date" value={form.saleEnds} onChange={e => set("saleEnds", e.target.value)}/></label>
          <label className="ob-field"><span>Compare-at ({unit})</span><input inputMode="decimal" value={form.compareAt} placeholder="Optional" onChange={e => set("compareAt", e.target.value)}/><small className="ob-field-hint">A struck-through price, above the price buyers pay.</small></label>
          <p className="ob-shipping-note ob-field-wide">Leave the sale empty to sell at the regular price. Buyers pay the sale price on those days, and it is signed with the listing.</p>
          <label className="ob-default-address"><input type="checkbox" checked={form.acceptsOffers} onChange={e => set("acceptsOffers", e.target.checked)}/>Accept offers</label>
          <label className="ob-field"><span>Lot size</span><input inputMode="numeric" value={form.lotSize} placeholder="1" onChange={e => set("lotSize", e.target.value)}/><small className="ob-field-hint">Pieces in one unit. One is a single piece.</small></label>
          <label className="ob-field"><span>Availability</span><select value={form.availability.kind} onChange={e => set("availability", { ...form.availability, kind: e.target.value })}><option value="in-stock">In stock</option><option value="pre-order">Pre-order</option><option value="made-to-order">Made to order</option><option value="away">Away</option></select></label>
          {form.availability.kind === "pre-order" && <label className="ob-field"><span>Ships on</span><input type="date" value={form.availability.shipsOn} onChange={e => set("availability", { ...form.availability, shipsOn: e.target.value })}/></label>}
          {form.availability.kind === "made-to-order" && <label className="ob-field"><span>Ready in (days)</span><input inputMode="numeric" value={form.availability.days} placeholder="14" onChange={e => set("availability", { ...form.availability, days: e.target.value })}/></label>}
          {form.tiers.map((tier, index) => <div className="ob-service-row ob-volume-tier ob-field-wide" key={index}>
            <label className="ob-field"><span>Buy at least</span><input inputMode="numeric" value={tier.qty} placeholder="3" onChange={e => set("tiers", form.tiers.map((item, at) => at === index ? { ...item, qty: e.target.value } : item))}/></label>
            <label className="ob-field"><span>Each ({unit})</span><input inputMode="decimal" value={tier.price} placeholder="20.00" onChange={e => set("tiers", form.tiers.map((item, at) => at === index ? { ...item, price: e.target.value } : item))}/></label>
            {form.tiers.length > 1 && <button type="button" className="ob-text-link" onClick={() => set("tiers", form.tiers.filter((_, at) => at !== index))}>Remove</button>}
          </div>)}
          {form.tiers.length < 4 && <button type="button" className="ob-secondary" onClick={() => set("tiers", [...form.tiers, { qty: "", price: "" }])}>Add a volume tier</button>}
          <p className="ob-shipping-note ob-field-wide">A blank tier row is left off the listing. Each price is below the regular price, and each quantity is higher than the one before it.</p>
        </>}
      </div>
      {!form.commerce && <div className="ob-listing-configuration">
        <div><strong>Specifications and fulfillment</strong><p>{form.currency === "SAT" ? "Choose a fiat price currency to use variants and service-based shipping. Customers still pay in bitcoin." : "Configure item condition, variants, SKUs, package dimensions, delivery services and returns."}</p></div>
        <button type="button" className="ob-secondary" disabled={form.currency === "SAT"} onClick={() => configure()}>Configure fulfillment</button>
      </div>}
    </fieldset>
    {form.commerce && <CommerceFields commerce={form.commerce} kind={form.kind} currency={form.currency === "SAT" ? "USD" : form.currency} basePrice={toMinorUnits(form.price, form.currency) ?? 0} images={form.images} packageFields={form} onPackage={patch => setForm(current => ({ ...current, ...patch }))} specifics={form.specifics} onSpecifics={specifics => set("specifics", specifics)} returns={form.returns} onReturns={patch => setForm(current => ({ ...current, returns: { ...current.returns, ...patch } }))} variantWeights={form.variantWeights} onVariantWeights={variantWeights => set("variantWeights", variantWeights)} onChange={c => set("commerce", c)}/>}
    <fieldset className="ob-store-section" id="ob-listing-content">
      <legend>Content rating</legend>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Rating</span><select value={form.contentRating} onChange={e => set("contentRating", e.target.value as ListingDraft["contentRating"])}><option value="general">General</option><option value="sensitive">Sensitive</option><option value="adult">Adult</option></select></label>
        <div className="ob-field"><span>Contains</span><div className="ob-tag-choice">{CONTENT_TAGS.map(tag => <label key={tag}><input type="checkbox" checked={form.contentTags.includes(tag)} onChange={e => set("contentTags", e.target.checked ? [...form.contentTags, tag] : form.contentTags.filter(t => t !== tag))}/>{tag.charAt(0).toUpperCase() + tag.slice(1)}</label>)}</div></div>
      </div>
    </fieldset>
    {listing?.price && <p className="ob-page-intro">Signed at {formatPrice(listing.price.amount, listing.price.currency)} ({listing.priceSats.toLocaleString("en-US")} sats that day).</p>}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions">
      <button type="button" className="ob-secondary" onClick={onCancel}>Cancel</button>
      <button type="submit" data-action="draft" className="ob-secondary" disabled={busy}>Save draft</button>
      <button type="submit" data-action="publish" className="ob-primary" disabled={busy}>{busy ? "Saving…" : listing ? "Save changes" : "Publish listing"}</button>
    </div>
    {dialog}
  </form>;
}
