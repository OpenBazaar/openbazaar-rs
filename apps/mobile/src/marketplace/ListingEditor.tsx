import { useEffect, useState, type FormEvent } from "react";
import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import type { Commerce } from "../commerce";
import { CONTENT_TAGS, MAX_LISTING_IMAGES, PRICE_CURRENCIES, deleteDraft, draftOf, formatPrice, fromMinorUnits, saveDraft, saveListing, toMinorUnits, type ListingDraft, type PriceCurrency, type SavedDraft } from "../seller";
import { ImageUpload, MediaImage, useLockedAction } from "./SellerKit";
import { TAXONOMY, axesProblem, compareAtText, emptyCommerce, listingFacts, packageFieldsOf, priceTerms, purchaseFields, purchaseTerms, returnsDraft, rowsOf, saleFieldsOf, scheduledSale, shippingTerms, specificRows, tierRows, volumeTiers, withPackage, withVariantWeights, type AvailabilityDraft, type ReturnsDraft, type SpecificRow, type TierRow } from "../commerceDraft";
import { draftListingId } from "../listingDraft";
import { categoryFieldsOf, filledSpecifics, filledTiers, signedCategory } from "../listingFields";
import { ItemDetails, OptionFields, ReturnsFields, ShippingServices, type PackageFields } from "./CommerceFields";
import { CountrySelect, Field, Reveal, WholeNumber, type Problem } from "./EditorFields";

type Form = Omit<ListingDraft, "price" | "shipping" | "stock"> & PackageFields & { price: string; shipping: string; stock: string; salePrice: string; saleStarts: string; saleEnds: string; compareAt: string; acceptsOffers: boolean; lotSize: string; availability: AvailabilityDraft; tiers: TierRow[]; variantWeights: string[]; taxonomy: string; subcategory: string; specifics: SpecificRow[]; returns: ReturnsDraft };

const EMPTY: ListingDraft = { title: "", description: "", category: "", kind: "physical", shipFrom: "", stock: 1, images: [], currency: "USD", price: 0, shipping: 0, contentRating: "general", contentTags: [] };
const MAX_TIERS = 4;

// Section ids, in page order, and the section each field sits in.
const BASICS = "ob-listing-details", PHOTOS = "ob-listing-photos", PRICING = "ob-listing-pricing", DISCOUNTS = "ob-listing-discounts", ITEM = "ob-listing-item", OPTIONS = "ob-listing-options", SHIPPING = "ob-listing-shipping", RETURNS = "ob-listing-returns", CONTENT = "ob-listing-content";
const TITLE = "ob-f-title", CATEGORY = "ob-f-category", SUBCATEGORY = "ob-f-subcategory", CURRENCY = "ob-f-currency", PRICE = "ob-f-price", STOCK = "ob-f-stock", SHIP_FROM = "ob-f-ship-from", FLAT_SHIPPING = "ob-f-shipping";
const SECTION_OF: Record<string, string> = { [TITLE]: BASICS, [CATEGORY]: BASICS, [SUBCATEGORY]: BASICS, [CURRENCY]: PRICING, [PRICE]: PRICING, [STOCK]: PRICING, [SHIP_FROM]: SHIPPING, [FLAT_SHIPPING]: SHIPPING };

function formOf(draft: ListingDraft): Form {
  const sale = saleFieldsOf(draft.price, draft.commerce?.sale ?? null, draft.currency);
  const purchase = purchaseFields(draft.commerce);
  const rows = draft.commerce ? rowsOf(draft.commerce) : [];
  return { ...draft, ...packageFieldsOf(draft.commerce?.shipping, draft.currency), ...categoryFieldsOf(draft.category, draft.commerce?.taxonomy ?? 0), specifics: filledSpecifics(specificRows(draft.commerce?.specifics)), returns: returnsDraft(draft.commerce?.returns), price: draft.price ? fromMinorUnits(draft.price, draft.currency) : "", shipping: draft.shipping ? fromMinorUnits(draft.shipping, draft.currency) : "", stock: String(draft.stock), salePrice: sale.price, saleStarts: sale.startsOn, saleEnds: sale.endsOn, compareAt: compareAtText(draft.commerce?.compareAt ?? 0, draft.currency), acceptsOffers: draft.commerce?.acceptsOffers ?? false, lotSize: purchase.lotSize, availability: purchase.availability, tiers: filledTiers(tierRows(draft.price, draft.commerce?.tiers ?? [], draft.currency)), variantWeights: rows.map(row => row.weightG ? String(row.weightG) : "") };
}

/// The draft the node takes, or what to fix first and where.
function draftFrom(form: Form): ListingDraft | Problem {
  const fail = (at: string, message: string): Problem => ({ at, message });
  const fiat = form.currency !== "SAT";
  const { salePrice: _salePrice, saleStarts: _saleStarts, saleEnds: _saleEnds, compareAt: _compareAt, acceptsOffers: _acceptsOffers, lotSize: _lotSize, availability: _availability, tiers: _tiers, variantWeights: _variantWeights, taxonomy: _taxonomy, subcategory: _subcategory, weightG: _weightG, lengthMm: _lengthMm, widthMm: _widthMm, heightMm: _heightMm, handlingDays: _handlingDays, freeOver: _freeOver, specifics: _specifics, returns: _returns, ...rest } = form;
  if (!form.title.trim()) return fail(TITLE, "Give the listing a title.");
  const category = signedCategory(form, fiat);
  if (typeof category === "string") return fail(category.startsWith("A subcategory") ? SUBCATEGORY : CATEGORY, category);
  const price = toMinorUnits(form.price, form.currency);
  if (!price) return fail(PRICE, fiat ? `Enter a price in ${form.currency}, like 12.50.` : "Enter a price in whole sats.");
  const stock = Number(form.stock);
  if (!Number.isInteger(stock) || stock < 0 || stock > 1_000_000) return fail(STOCK, "Enter how many you have, from 0 to 1,000,000.");
  if (form.kind === "physical" && !form.shipFrom.trim()) return fail(SHIP_FROM, "Choose the country it ships from.");
  const shipping = form.kind === "digital" || !form.shipping.trim() ? 0 : toMinorUnits(form.shipping, form.currency);
  if (shipping === null) return fail(FLAT_SHIPPING, "Enter shipping as a plain amount, or leave it empty for free shipping.");
  if (!fiat) {
    // A sats price is signed without the version 7 block, so the terms that live there are left off.
    if (form.commerce?.variants.length || form.commerce?.shipping.services.length) return fail(CURRENCY, "Options and shipping services need a price in USD, EUR, GBP or JPY. Switch the price currency back, or remove them first.");
    return { ...rest, category: category.category, price, shipping, stock, commerce: null };
  }
  let commerce: Commerce = { ...(form.commerce ?? emptyCommerce()), taxonomy: category.taxonomy };
  const sale = scheduledSale(price, form.currency, { price: form.salePrice, startsOn: form.saleStarts, endsOn: form.saleEnds });
  if (typeof sale === "string") return fail(DISCOUNTS, sale);
  commerce = { ...commerce, sale };
  if (form.kind === "physical") {
    const terms = shippingTerms(form.currency, {
      weightG: form.weightG, lengthMm: form.lengthMm, widthMm: form.widthMm, heightMm: form.heightMm,
      handlingDays: form.handlingDays, freeOver: form.freeOver,
      regions: commerce.shipping.services.map(service => service.regions.join(", ")),
    });
    if (typeof terms === "string") return fail(SHIPPING, terms);
    commerce = withPackage(commerce, terms) ?? commerce;
    const names = commerce.shipping.services.map(service => service.name.trim().toLowerCase());
    if (commerce.shipping.services.some(s => !s.name.trim() || s.daysMin > s.daysMax)) return fail(SHIPPING, "Name each shipping service and give its delivery days, fastest first.");
    if (new Set(names).size !== names.length) return fail(SHIPPING, "Give each shipping service its own name.");
    if (commerce.shipping.services.some(s => s.extraItem > s.price)) return fail(SHIPPING, "An extra item costs no more to ship than the first.");
  }
  const facts = listingFacts(form.specifics, form.returns);
  if (typeof facts === "string") return fail(facts.startsWith("A returns") ? RETURNS : ITEM, facts);
  commerce = { ...commerce, specifics: facts.specifics, returns: facts.returns };
  const priced = priceTerms(price, form.currency, form.compareAt, form.acceptsOffers, commerce.variants.map(variant => variant.price));
  if (typeof priced === "string") return fail(DISCOUNTS, priced);
  commerce = { ...commerce, compareAt: priced.compareAt, acceptsOffers: priced.acceptsOffers };
  const purchase = purchaseTerms(form.lotSize, form.availability);
  if (typeof purchase === "string") return fail(PRICING, purchase);
  commerce = { ...commerce, lotSize: purchase.lotSize, availability: purchase.availability };
  const tiers = volumeTiers(price, form.currency, form.tiers);
  if (typeof tiers === "string") return fail(DISCOUNTS, tiers);
  commerce = { ...commerce, tiers };
  const rows = rowsOf(commerce);
  if (commerce.variants.length) {
    const weights = form.variantWeights.length === rows.length ? rows.flatMap((row, index) => (row.offered ? [form.variantWeights[index] ?? ""] : [])) : form.variantWeights;
    const variants = withVariantWeights(commerce.variants, weights);
    if (typeof variants === "string") return fail(OPTIONS, variants);
    commerce = { ...commerce, variants };
  }
  const problem = axesProblem(commerce.axes, rows);
  if (problem) return fail(OPTIONS, problem);
  return { ...rest, category: category.category, price, shipping, stock, commerce };
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
  const [error, setError] = useState<Problem | null>(null);
  const [saleOn, setSaleOn] = useState(() => Boolean(form.salePrice || form.saleStarts || form.saleEnds));
  const [compareOn, setCompareOn] = useState(() => Boolean(form.compareAt));
  const { run, dialog } = useLockedAction(message => setError(message ? { at: "", message } : null));
  const patch = (change: Partial<Form>) => { setError(null); setForm(f => ({ ...f, ...change })); };
  const set = <K extends keyof Form>(key: K, value: Form[K]) => patch({ [key]: value } as unknown as Partial<Form>);
  // Take the seller to the field or section the editor refused.
  useEffect(() => {
    const target = error?.at ? document.getElementById(error.at) : null;
    if (!target) return;
    target.scrollIntoView({ block: "center" });
    if (target instanceof HTMLInputElement || target instanceof HTMLSelectElement || target instanceof HTMLTextAreaElement) target.focus({ preventScroll: true });
  }, [error]);
  async function save(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    const publish = (e.nativeEvent as SubmitEvent).submitter?.getAttribute("data-action") !== "draft";
    const listingId = draftListingId(listing, savedDraft);
    const draft = draftFrom(form);
    if ("message" in draft) {
      setError(draft);
      return;
    }
    setBusy(true);
    setError(null);
    await run(async () => {
      if (!publish) {
        const saved = await saveDraft(draftId, { ...draft, listingId });
        setDraftId(saved.draftId);
        onSaved("Draft saved");
        return;
      }
      await saveListing(listingId, draft);
      if (draftId) await deleteDraft(draftId);
      onSaved(listing ? "Listing updated and signed" : "Listing published");
    });
    setBusy(false);
  }
  const fiat = form.currency !== "SAT";
  const physical = form.kind === "physical";
  const unit = fiat ? form.currency : "sats";
  const commerce = form.commerce ?? emptyCommerce();
  const setCommerce = (next: Commerce) => set("commerce", next);
  const variants = fiat && commerce.variants.length > 0;
  const services = fiat && commerce.shipping.services.length > 0;
  const at = (id: string) => (error?.at === id ? error.message : null);
  const errorSection = error ? SECTION_OF[error.at] ?? error.at : "";
  const sections: [string, string][] = [[BASICS, "Basics"], [PHOTOS, "Photos"], [PRICING, "Price and stock"]];
  if (fiat) sections.push([DISCOUNTS, "Offers and discounts"], [ITEM, "Item details"], [OPTIONS, "Options"]);
  if (physical) sections.push([SHIPPING, "Shipping"]);
  if (fiat) sections.push([RETURNS, "Returns"]);
  sections.push([CONTENT, "Content rating"]);
  const sectionError = (id: string) => at(id) && <p className="ob-section-error">{at(id)}</p>;
  return <form id="ob-listing-form" className="ob-listing-editor" noValidate onSubmit={e => void save(e)} aria-label={listing ? `Edit ${listing.title}` : "New listing"}>
    <nav className="ob-editor-sections" aria-label="Listing sections">
      {sections.map(([id, label]) => <a key={id} href={`#${id}`} className={errorSection === id ? "has-error" : undefined}>{label}</a>)}
    </nav>

    <fieldset className="ob-store-section" id={BASICS}>
      <legend>Basics</legend>
      <div className="ob-field-grid">
        <Field label="Title" wide error={at(TITLE)}><input id={TITLE} value={form.title} maxLength={120} aria-invalid={at(TITLE) ? true : undefined} onChange={e => set("title", e.target.value)}/></Field>
        <Field label="Description" wide><textarea value={form.description} maxLength={5000} onChange={e => set("description", e.target.value)}/></Field>
        <div className="ob-field ob-field-wide" role="radiogroup" aria-label="Type of good"><span>Type of good</span><div className="ob-kind-choice">
          {(["physical", "digital"] as const).map(kind => <label key={kind} className={form.kind === kind ? "is-selected" : ""}><input type="radio" name="kind" checked={form.kind === kind} onChange={() => set("kind", kind)}/>{kind === "physical" ? "Physical item" : "Digital good"}</label>)}
        </div></div>
        <Field label="Category" error={at(CATEGORY)}>
          <select id={CATEGORY} value={form.taxonomy} aria-invalid={at(CATEGORY) ? true : undefined} onChange={e => set("taxonomy", e.target.value)}>
            <option value="" disabled>Choose a category…</option>
            {TAXONOMY.map(entry => <option key={entry.id} value={String(entry.id)}>{entry.name}</option>)}
          </select>
        </Field>
        {fiat && <Field label="Subcategory" hint="Optional. A few words buyers would search for, such as “Wool blankets”." error={at(SUBCATEGORY)}><input id={SUBCATEGORY} value={form.subcategory} maxLength={40} autoCapitalize="off" aria-invalid={at(SUBCATEGORY) ? true : undefined} onChange={e => set("subcategory", e.target.value)}/></Field>}
      </div>
    </fieldset>

    <fieldset className="ob-store-section" id={PHOTOS} aria-label="Product media">
      <legend>Photos</legend>
      <p className="ob-field-hint">Up to {MAX_LISTING_IMAGES}. The first photo is the cover.</p>
      <div className="ob-listing-photos">
        {form.images.map((hash, i) => <figure key={hash} className="ob-listing-photo">
          <MediaImage hash={hash} alt={`Photo ${i + 1}`}/>
          <button type="button" className="ob-icon-button" aria-label={`Remove photo ${i + 1}`} onClick={() => set("images", form.images.filter(h => h !== hash))}>×</button>
        </figure>)}
        {form.images.length < MAX_LISTING_IMAGES && <ImageUpload label="Add photos" multiple onUploaded={hash => setForm(f => f.images.includes(hash) || f.images.length >= MAX_LISTING_IMAGES ? f : { ...f, images: [...f.images, hash] })}/>}
      </div>
    </fieldset>

    <fieldset className="ob-store-section" id={PRICING}>
      <legend>Price and stock</legend>
      {sectionError(PRICING)}
      <div className="ob-field-grid">
        <Field label="Price currency" error={at(CURRENCY)} hint={fiat ? null : "Sales, options, shipping services and returns need USD, EUR, GBP or JPY. Buyers pay in bitcoin either way."}><select id={CURRENCY} value={form.currency} onChange={e => set("currency", e.target.value as PriceCurrency)}>{PRICE_CURRENCIES.map(c => <option key={c} value={c}>{c === "SAT" ? "Bitcoin (sats)" : c}</option>)}</select></Field>
        <Field label={`Price (${unit})`} error={at(PRICE)} hint={satsHint(toMinorUnits(form.price, form.currency), form.currency, rates) ?? (fiat ? "Buyers pay the sats this is worth when they order." : "Buyers pay exactly this many sats.")}><input id={PRICE} inputMode="decimal" value={form.price} aria-invalid={at(PRICE) ? true : undefined} onChange={e => set("price", e.target.value)}/></Field>
        {variants ? <p className="ob-shipping-note">Stock is set for each variant under <a href={`#${OPTIONS}`}>Options</a>.</p> : <Field label="In stock" error={at(STOCK)}><WholeNumber id={STOCK} value={form.stock} max={1_000_000} invalid={Boolean(at(STOCK))} onChange={stock => set("stock", stock)}/></Field>}
        {fiat && <>
          <Field label="Availability"><select value={form.availability.kind} onChange={e => set("availability", { ...form.availability, kind: e.target.value })}><option value="in-stock">In stock</option><option value="pre-order">Pre-order</option><option value="made-to-order">Made to order</option><option value="away">Away</option></select></Field>
          {form.availability.kind === "pre-order" && <Field label="Ships on"><input type="date" value={form.availability.shipsOn} onChange={e => set("availability", { ...form.availability, shipsOn: e.target.value })}/></Field>}
          {form.availability.kind === "made-to-order" && <Field label="Ready in (days)" hint="At most 365."><WholeNumber value={form.availability.days} max={365} onChange={days => set("availability", { ...form.availability, days })}/></Field>}
          <Field label="Lot size" hint="Pieces sold together as one unit. One is a single piece."><WholeNumber value={form.lotSize} min={1} max={65_535} onChange={lotSize => set("lotSize", lotSize)}/></Field>
          <Field label="Most per order" hint="Leave empty for no limit."><WholeNumber value={commerce.maxPerOrder ? String(commerce.maxPerOrder) : ""} min={1} max={999} onChange={value => setCommerce({ ...commerce, maxPerOrder: Math.min(999, Number(value) || 0) })}/></Field>
        </>}
      </div>
    </fieldset>

    {fiat && <fieldset className="ob-store-section" id={DISCOUNTS}>
      <legend>Offers and discounts</legend>
      {sectionError(DISCOUNTS)}
      <Reveal label="Accept offers" hint="Buyers can propose a price. You accept or decline each one." on={form.acceptsOffers} onToggle={on => set("acceptsOffers", on)}/>
      <Reveal label="Run a sale" hint="Buyers pay the sale price on the days you choose." on={saleOn} onToggle={on => { setSaleOn(on); if (!on) patch({ salePrice: "", saleStarts: "", saleEnds: "" }); }}>
        <div className="ob-field-grid ob-field-grid-3">
          <Field label={`Sale price (${unit})`} hint="Below the regular price."><input inputMode="decimal" value={form.salePrice} onChange={e => set("salePrice", e.target.value)}/></Field>
          <Field label="Sale starts"><input type="date" value={form.saleStarts} onChange={e => set("saleStarts", e.target.value)}/></Field>
          <Field label="Sale ends"><input type="date" value={form.saleEnds} min={form.saleStarts || undefined} onChange={e => set("saleEnds", e.target.value)}/></Field>
        </div>
      </Reveal>
      <Reveal label="Show a compare-at price" hint="A struck-through price shown above the price buyers pay." on={compareOn} onToggle={on => { setCompareOn(on); if (!on) set("compareAt", ""); }}>
        <div className="ob-field-grid"><Field label={`Compare-at (${unit})`} hint="Above every price on this listing."><input inputMode="decimal" value={form.compareAt} onChange={e => set("compareAt", e.target.value)}/></Field></div>
      </Reveal>
      <Reveal label="Volume pricing" hint="A lower price for each piece when a buyer orders more." on={form.tiers.length > 0} onToggle={on => set("tiers", on ? [{ qty: "", price: "" }] : [])}>
        {form.tiers.map((tier, index) => <div className="ob-service-row ob-volume-tier" key={index}>
          <Field label="Buy at least" hint="Two or more."><WholeNumber value={tier.qty} min={2} max={65_535} onChange={qty => set("tiers", form.tiers.map((item, n) => n === index ? { ...item, qty } : item))}/></Field>
          <Field label={`Each (${unit})`} hint="Below the regular price."><input inputMode="decimal" value={tier.price} onChange={e => set("tiers", form.tiers.map((item, n) => n === index ? { ...item, price: e.target.value } : item))}/></Field>
          <button type="button" className="ob-text-link" aria-label={`Remove volume tier ${index + 1}`} onClick={() => set("tiers", form.tiers.filter((_, n) => n !== index))}>Remove</button>
        </div>)}
        {form.tiers.length < MAX_TIERS && <button type="button" className="ob-secondary" onClick={() => set("tiers", [...form.tiers, { qty: "", price: "" }])}>Add a volume tier</button>}
        <p className="ob-shipping-note">Each tier needs a higher quantity and a lower price than the one before it.</p>
      </Reveal>
    </fieldset>}

    {fiat && <fieldset className="ob-store-section" id={ITEM}>
      <legend>Item details</legend>
      {sectionError(ITEM)}
      <ItemDetails commerce={commerce} onChange={setCommerce} specifics={form.specifics} onSpecifics={specifics => set("specifics", specifics)}/>
    </fieldset>}

    {fiat && <fieldset className="ob-store-section" id={OPTIONS}>
      <legend>Options</legend>
      {sectionError(OPTIONS)}
      <OptionFields commerce={commerce} onChange={setCommerce} currency={form.currency} basePrice={toMinorUnits(form.price, form.currency) ?? 0} images={form.images} variantWeights={form.variantWeights} onVariantWeights={variantWeights => set("variantWeights", variantWeights)}/>
    </fieldset>}

    {physical && <fieldset className="ob-store-section" id={SHIPPING}>
      <legend>Shipping</legend>
      {sectionError(SHIPPING)}
      <div className="ob-field-grid">
        <CountrySelect id={SHIP_FROM} label="Ships from" value={form.shipFrom} error={at(SHIP_FROM)} hint="Only the country is published." onChange={name => set("shipFrom", name)}/>
        {!services && <Field label={`Shipping (${unit})`} error={at(FLAT_SHIPPING)} hint="One flat price for a package. Leave empty for free shipping."><input id={FLAT_SHIPPING} inputMode="decimal" value={form.shipping} aria-invalid={at(FLAT_SHIPPING) ? true : undefined} onChange={e => set("shipping", e.target.value)}/></Field>}
      </div>
      {fiat && <ShippingServices commerce={commerce} onChange={setCommerce} currency={form.currency} packageFields={form} onPackage={change => patch(change)}/>}
    </fieldset>}

    {fiat && <fieldset className="ob-store-section" id={RETURNS}>
      <legend>Returns</legend>
      {sectionError(RETURNS)}
      <ReturnsFields returns={form.returns} onReturns={change => patch({ returns: { ...form.returns, ...change } })}/>
    </fieldset>}

    <fieldset className="ob-store-section" id={CONTENT}>
      <legend>Content rating</legend>
      <div className="ob-field-grid">
        <Field label="Rating"><select value={form.contentRating} onChange={e => set("contentRating", e.target.value as ListingDraft["contentRating"])}><option value="general">General</option><option value="sensitive">Sensitive</option><option value="adult">Adult</option></select></Field>
        <div className="ob-field" role="group" aria-label="Contains"><span>Contains</span><div className="ob-tag-choice">{CONTENT_TAGS.map(tag => <label key={tag}><input type="checkbox" checked={form.contentTags.includes(tag)} onChange={e => set("contentTags", e.target.checked ? [...form.contentTags, tag] : form.contentTags.filter(t => t !== tag))}/>{tag.charAt(0).toUpperCase() + tag.slice(1)}</label>)}</div></div>
      </div>
    </fieldset>

    <div className="ob-editor-actions">
      {listing?.price && <p className="ob-page-intro">Signed at {formatPrice(listing.price.amount, listing.price.currency)} ({listing.priceSats.toLocaleString("en-US")} sats that day).</p>}
      {error && <p className="ob-form-error" role="alert">{error.message}</p>}
      <div className="ob-form-actions">
        <button type="button" className="ob-secondary" onClick={onCancel}>Cancel</button>
        <button type="submit" data-action="draft" className="ob-secondary" disabled={busy}>Save draft</button>
        <button type="submit" data-action="publish" className="ob-primary" disabled={busy}>{busy ? "Saving…" : listing ? "Save changes" : "Publish listing"}</button>
      </div>
    </div>
    {dialog}
  </form>;
}
