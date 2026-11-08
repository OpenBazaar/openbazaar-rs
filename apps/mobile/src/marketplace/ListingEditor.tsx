import { useState, type FormEvent } from "react";
import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import { CONTENT_TAGS, MAX_LISTING_IMAGES, PRICE_CURRENCIES, draftOf, formatPrice, fromMinorUnits, saveListing, toMinorUnits, type ListingDraft, type PriceCurrency } from "../seller";
import { ImageUpload, MediaImage, useLockedAction } from "./SellerKit";
import { axesProblem, compareAtText, emptyCommerce, listingFacts, packageFieldsOf, priceTerms, returnsDraft, rowsOf, saleFieldsOf, scheduledSale, shippingTerms, specificRows, withFacts, withPackage, withPrice, withSale, type PackageInput, type ReturnsDraft, type SpecificRow } from "../commerceDraft";
import { CommerceFields } from "./CommerceFields";

type PackageFields = Omit<PackageInput, "regions">;
type Form = Omit<ListingDraft, "price" | "shipping" | "stock"> & PackageFields & { price: string; shipping: string; stock: string; salePrice: string; saleStarts: string; saleEnds: string; compareAt: string; acceptsOffers: boolean; specifics: SpecificRow[]; returns: ReturnsDraft };

const EMPTY: ListingDraft = { title: "", description: "", category: "", kind: "physical", shipFrom: "", stock: 1, images: [], currency: "USD", price: 0, shipping: 0, contentRating: "general", contentTags: [] };

function formOf(draft: ListingDraft): Form {
  const sale = saleFieldsOf(draft.price, draft.commerce?.sale ?? null, draft.currency);
  return { ...draft, ...packageFieldsOf(draft.commerce?.shipping, draft.currency), specifics: specificRows(draft.commerce?.specifics), returns: returnsDraft(draft.commerce?.returns), price: draft.price ? fromMinorUnits(draft.price, draft.currency) : "", shipping: draft.shipping ? fromMinorUnits(draft.shipping, draft.currency) : "", stock: String(draft.stock), salePrice: sale.price, saleStarts: sale.startsOn, saleEnds: sale.endsOn, compareAt: compareAtText(draft.commerce?.compareAt ?? 0, draft.currency), acceptsOffers: draft.commerce?.acceptsOffers ?? false };
}

/// The draft the node takes, or what to fix first.
function draftFrom(form: Form): ListingDraft | string {
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
  if (commerce && form.currency === "SAT") return "Price a listing with options or shipping services in USD, EUR, GBP, or JPY.";
  if (commerce) {
    const problem = axesProblem(commerce.axes, rowsOf(commerce));
    if (problem) return problem;
    if (commerce.shipping.services.some(s => !s.name.trim() || s.daysMin > s.daysMax)) return "Name each shipping service and give its delivery days, fastest first.";
  }
  const { salePrice: _salePrice, saleStarts: _saleStarts, saleEnds: _saleEnds, compareAt: _compareAt, acceptsOffers: _acceptsOffers, weightG: _weightG, lengthMm: _lengthMm, widthMm: _widthMm, heightMm: _heightMm, handlingDays: _handlingDays, freeOver: _freeOver, specifics: _specifics, returns: _returns, ...rest } = form;
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

export function ListingEditor({ listing, categories, rates, onSaved, onCancel }: { listing: ListingHeader | null; categories: string[]; rates: RateTable; onSaved: (message: string) => void; onCancel: () => void }) {
  const [form, setForm] = useState<Form>(() => formOf(listing ? draftOf(listing) : EMPTY));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const set = <K extends keyof Form>(key: K, value: Form[K]) => setForm(f => ({ ...f, [key]: value }));
  async function save(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    const draft = draftFrom(form);
    if (typeof draft === "string") {
      setError(draft);
      return;
    }
    setBusy(true);
    setError("");
    await run(async () => {
      await saveListing(listing?.listingId ?? null, draft);
      onSaved(listing ? "Listing updated and signed" : "Listing published");
    });
    setBusy(false);
  }
  const unit = form.currency === "SAT" ? "sats" : form.currency;
  const variants = (form.commerce?.variants.length ?? 0) > 0;
  const services = (form.commerce?.shipping.services.length ?? 0) > 0;
  return <form className="ob-listing-editor" onSubmit={e => void save(e)} aria-label={listing ? `Edit ${listing.title}` : "New listing"}>
    <fieldset className="ob-store-section">
      <legend>Photos</legend>
      <div className="ob-listing-photos">
        {form.images.map((hash, i) => <figure key={hash} className="ob-listing-photo">
          <MediaImage hash={hash} alt={`Photo ${i + 1}`}/>
          <button type="button" className="ob-icon-button" aria-label={`Remove photo ${i + 1}`} onClick={() => set("images", form.images.filter(h => h !== hash))}>×</button>
        </figure>)}
        {form.images.length < MAX_LISTING_IMAGES && <ImageUpload label="Add photos" multiple onUploaded={hash => setForm(f => f.images.includes(hash) || f.images.length >= MAX_LISTING_IMAGES ? f : { ...f, images: [...f.images, hash] })}/>}
      </div>
    </fieldset>
    <fieldset className="ob-store-section">
      <legend>Details</legend>
      <div className="ob-field-grid">
        <label className="ob-field ob-field-wide"><span>Title</span><input value={form.title} maxLength={120} onChange={e => set("title", e.target.value)}/></label>
        <label className="ob-field ob-field-wide"><span>Description</span><textarea value={form.description} maxLength={5000} onChange={e => set("description", e.target.value)}/></label>
        <label className="ob-field"><span>Category</span><input list="ob-listing-categories" value={form.category} maxLength={40} placeholder="Home" onChange={e => set("category", e.target.value)}/><datalist id="ob-listing-categories">{categories.map(c => <option key={c} value={c}/>)}</datalist></label>
        <div className="ob-field" role="radiogroup" aria-label="Type of good"><span>Type of good</span><div className="ob-kind-choice">
          {(["physical", "digital"] as const).map(kind => <label key={kind} className={form.kind === kind ? "is-selected" : ""}><input type="radio" name="kind" checked={form.kind === kind} onChange={() => set("kind", kind)}/>{kind === "physical" ? "Physical item" : "Digital good"}</label>)}
        </div></div>
      </div>
    </fieldset>
    <fieldset className="ob-store-section">
      <legend>Price and stock</legend>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Price currency</span><select value={form.currency} onChange={e => set("currency", e.target.value as PriceCurrency)}>{PRICE_CURRENCIES.map(c => <option key={c} value={c}>{c === "SAT" ? "Bitcoin (sats)" : c}</option>)}</select></label>
        <label className="ob-field"><span>Price ({unit})</span><input inputMode="decimal" value={form.price} placeholder={form.currency === "SAT" ? "25000" : "12.50"} onChange={e => set("price", e.target.value)}/><small className="ob-field-hint">{satsHint(toMinorUnits(form.price, form.currency), form.currency, rates) ?? (form.currency === "SAT" ? "Buyers pay exactly this many sats." : "Buyers pay the sats this is worth when they order.")}</small></label>
        {form.kind === "physical" && !services && <label className="ob-field"><span>Shipping ({unit})</span><input inputMode="decimal" value={form.shipping} placeholder="Free" onChange={e => set("shipping", e.target.value)}/></label>}
        {form.kind === "physical" && <label className="ob-field"><span>Ships from</span><input value={form.shipFrom} maxLength={40} placeholder="US" onChange={e => set("shipFrom", e.target.value)}/></label>}
        {!variants && <label className="ob-field"><span>In stock</span><input inputMode="numeric" value={form.stock} onChange={e => set("stock", e.target.value)}/></label>}
        {form.currency !== "SAT" && <>
          <label className="ob-field"><span>Sale price ({unit})</span><input inputMode="decimal" value={form.salePrice} placeholder="Optional" onChange={e => set("salePrice", e.target.value)}/></label>
          <label className="ob-field"><span>Sale starts</span><input type="date" value={form.saleStarts} onChange={e => set("saleStarts", e.target.value)}/></label>
          <label className="ob-field"><span>Sale ends</span><input type="date" value={form.saleEnds} onChange={e => set("saleEnds", e.target.value)}/></label>
          <label className="ob-field"><span>Compare-at ({unit})</span><input inputMode="decimal" value={form.compareAt} placeholder="Optional" onChange={e => set("compareAt", e.target.value)}/><small className="ob-field-hint">A struck-through price, above the price buyers pay.</small></label>
          <p className="ob-shipping-note ob-field-wide">Leave the sale empty to sell at the regular price. Buyers pay the sale price on those days, and it is signed with the listing.</p>
          <label className="ob-default-address"><input type="checkbox" checked={form.acceptsOffers} onChange={e => set("acceptsOffers", e.target.checked)}/>Accept offers</label>
        </>}
      </div>
      <label className="ob-default-address"><input type="checkbox" checked={!!form.commerce} onChange={e => {
        if (!e.target.checked) { set("commerce", null); return; }
        const commerce = emptyCommerce();
        setForm(current => ({ ...current, commerce, ...packageFieldsOf(commerce.shipping, current.currency) }));
      }}/>Add condition, options like size or color, and shipping services</label>
    </fieldset>
    {form.commerce && <CommerceFields commerce={form.commerce} kind={form.kind} currency={form.currency === "SAT" ? "USD" : form.currency} basePrice={toMinorUnits(form.price, form.currency) ?? 0} images={form.images} packageFields={form} onPackage={patch => setForm(current => ({ ...current, ...patch }))} specifics={form.specifics} onSpecifics={specifics => set("specifics", specifics)} returns={form.returns} onReturns={patch => setForm(current => ({ ...current, returns: { ...current.returns, ...patch } }))} onChange={c => set("commerce", c)}/>}
    <fieldset className="ob-store-section">
      <legend>Content label</legend>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Rating</span><select value={form.contentRating} onChange={e => set("contentRating", e.target.value as ListingDraft["contentRating"])}><option value="general">General</option><option value="sensitive">Sensitive</option><option value="adult">Adult</option></select></label>
        <div className="ob-field"><span>Contains</span><div className="ob-tag-choice">{CONTENT_TAGS.map(tag => <label key={tag}><input type="checkbox" checked={form.contentTags.includes(tag)} onChange={e => set("contentTags", e.target.checked ? [...form.contentTags, tag] : form.contentTags.filter(t => t !== tag))}/>{tag.charAt(0).toUpperCase() + tag.slice(1)}</label>)}</div></div>
      </div>
    </fieldset>
    {listing?.price && <p className="ob-page-intro">Signed at {formatPrice(listing.price.amount, listing.price.currency)} ({listing.priceSats.toLocaleString("en-US")} sats that day).</p>}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions">
      <button type="button" className="ob-secondary" onClick={onCancel}>Cancel</button>
      <button className="ob-primary" disabled={busy}>{busy ? "Signing…" : listing ? "Save changes" : "Publish listing"}</button>
    </div>
    {dialog}
  </form>;
}
