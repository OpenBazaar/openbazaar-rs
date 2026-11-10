import { useState } from "react";
import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import { purchaseTerms, type AvailabilityDraft } from "../commerceDraft";
import { bulkEditListings, deleteListing, fetchDrafts, fetchOwnListings, formatPrice, type SavedDraft, type SellerProfile } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { ListingEditor } from "./ListingEditor";
import { Empty } from "./Marketplace";
import { PageHeading } from "./PageHeading";
import { MediaImage, useLockedAction } from "./SellerKit";
import { merchantBulkPrice } from "../bulkEdit";

/** Fiat estimate of a sat price, hidden when it repeats the currency the seller already set. */
function displayEstimate(sats: number, rates: RateTable, currency: CurrencyCode, priceCurrency: string | undefined) {
  if (priceCurrency && priceCurrency !== "SAT" && priceCurrency === currency) return null;
  const rate = rates[currency];
  if (!rate) return null;
  return new Intl.NumberFormat("en-US", { style: "currency", currency }).format((sats / 1e8) * rate);
}

/** The seller's own listings: add, edit, and remove. Each change is signed by the node. */
export function StoreListings({ profile, rates, currency, startNew = false, onChanged, onOpenSettings }: { profile: SellerProfile; rates: RateTable; currency: CurrencyCode; startNew?: boolean; onChanged: (message: string) => void; onOpenSettings: () => void }) {
  const resource = useNodeResource(() => fetchOwnListings(profile.storeKey), 0);
  const drafts = useNodeResource(() => fetchDrafts(), 0);
  const [editing, setEditing] = useState<ListingHeader | "new" | SavedDraft | null>(startNew && profile.setUp ? "new" : null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [bulk, setBulk] = useState<"price" | "availability">("price");
  const [bulkPrice, setBulkPrice] = useState("");
  const [bulkAvailability, setBulkAvailability] = useState<AvailabilityDraft>({ kind: "in-stock", shipsOn: "", days: "" });
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [stockFilter, setStockFilter] = useState("all");
  const { run, dialog } = useLockedAction(setError);
  const listings = resource.data ?? [];
  const selectedCurrencies = listings.filter(listing => selected.includes(listing.listingId ?? "")).map(listing => listing.price?.currency ?? "SAT");
  const priceCurrency = new Set(selectedCurrencies).size === 1 ? selectedCurrencies[0] : null;
  const search = query.trim().toLocaleLowerCase();
  const visible = listings.filter(listing => {
    const matches = !search || [listing.title, listing.category, ...(listing.commerce?.variants.map(variant => variant.sku) ?? [])].some(value => value.toLocaleLowerCase().includes(search));
    return matches && (stockFilter === "all" || (stockFilter === "available" ? listing.stock > 0 : stockFilter === "low" ? listing.stock > 0 && listing.stock <= 5 : listing.stock === 0));
  });
  async function changed(message: string) {
    setEditing(null);
    await resource.refresh();
    await drafts.refresh();
    onChanged(message);
  }
  function toggle(id: string) {
    setSelected(current => current.includes(id) ? current.filter(item => item !== id) : [...current, id]);
  }
  async function applyBulk() {
    const ids = selected.filter(id => listings.some(listing => listing.listingId === id));
    if (!ids.length) return;
    let change: Parameters<typeof bulkEditListings>[1];
    if (bulk === "price") {
      const priced = merchantBulkPrice(bulkPrice, selectedCurrencies);
      if (typeof priced === "string") {
        setError(priced);
        return;
      }
      change = priced;
    } else {
      const terms = purchaseTerms("1", bulkAvailability);
      if (typeof terms === "string") {
        setError(terms);
        return;
      }
      change = { availability: terms.availability };
    }
    setError("");
    await run(async () => {
      await bulkEditListings(ids, change);
      setSelected([]);
      await changed("Listings updated");
    });
  }
  async function remove(listing: ListingHeader) {
    if (!listing.listingId) return;
    const id = listing.listingId;
    setError("");
    await run(async () => {
      await deleteListing(id);
      setConfirm(null);
      await changed("Listing removed");
    });
  }
  const savedDraft = editing && editing !== "new" && "draftId" in editing ? editing : null;
  const published = editing && editing !== "new" && !("draftId" in editing) ? editing : null;
  if (editing) return <section className="ob-store-listings">
    <PageHeading title={editing === "new" ? "New listing" : savedDraft ? `Draft ${savedDraft.listing.title || "listing"}` : `Edit ${published?.title ?? "listing"}`} description="Price, stock, and where it ships from, signed with the listing"/>
    <ListingEditor listing={published} savedDraft={savedDraft} rates={rates} onSaved={message => void changed(message)} onCancel={() => setEditing(null)}/>
  </section>;
  return <section className="ob-store-listings">
    <PageHeading title="Your listings" description={profile.setUp ? `Goods on ${profile.name}, signed by your node and priced in sats` : "Open your store before a listing can be published. Store settings is under Settings."} action={profile.setUp ? <button className="ob-primary" onClick={() => setEditing("new")}>New listing</button> : <button className="ob-primary" onClick={onOpenSettings}>Go to store settings</button>}/>
    {listings.length > 0 && <div className="ob-inventory-toolbar">
      <label className="ob-field"><span>Search</span><input type="search" value={query} placeholder="Title, category, SKU" onChange={event => setQuery(event.target.value)}/></label>
      <label className="ob-field"><span>Stock</span><select value={stockFilter} onChange={event => setStockFilter(event.target.value)}><option value="all">All listings</option><option value="available">In stock</option><option value="low">Low, 1–5</option><option value="empty">Out of stock</option></select></label>
      <p role="status">{visible.length === listings.length ? `${listings.length} ${listings.length === 1 ? "listing" : "listings"}` : `${visible.length} of ${listings.length}`}{selected.length ? ` · ${selected.length} selected` : ""}</p>
    </div>}
    {(error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    {!!listings.length && <div className="ob-store-section">
      <p className="ob-shipping-note">One price or one availability, for the goods you select. A draft stays a draft.</p>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Change</span><select value={bulk} onChange={e => setBulk(e.target.value as "price" | "availability")}><option value="price">Regular price</option><option value="availability">Availability</option></select></label>
        {bulk === "price" ? <label className="ob-field"><span>Price{priceCurrency ? ` (${priceCurrency === "SAT" ? "sats" : priceCurrency})` : ""}</span><input inputMode="decimal" value={bulkPrice} disabled={!priceCurrency} placeholder={priceCurrency === "SAT" || priceCurrency === "JPY" ? "25000" : "25.00"} onChange={e => setBulkPrice(e.target.value)}/><small className="ob-field-hint">{priceCurrency ? "Replaces the price on each selected listing" : "Select listings priced in one currency"}</small></label> : <>
          <label className="ob-field"><span>Availability</span><select value={bulkAvailability.kind} onChange={e => setBulkAvailability({ ...bulkAvailability, kind: e.target.value })}><option value="in-stock">In stock</option><option value="pre-order">Pre-order</option><option value="made-to-order">Made to order</option><option value="away">Away</option></select></label>
          {bulkAvailability.kind === "pre-order" && <label className="ob-field"><span>Ships on</span><input type="date" value={bulkAvailability.shipsOn} onChange={e => setBulkAvailability({ ...bulkAvailability, shipsOn: e.target.value })}/></label>}
          {bulkAvailability.kind === "made-to-order" && <label className="ob-field"><span>Ready in (days)</span><input inputMode="numeric" value={bulkAvailability.days} placeholder="14" onChange={e => setBulkAvailability({ ...bulkAvailability, days: e.target.value })}/></label>}
        </>}
      </div>
      <button className="ob-secondary" disabled={!selected.length} onClick={() => void applyBulk()}>Apply to selected</button>
    </div>}
    {resource.loading ? <p role="status">Loading your listings…</p> : !profile.setUp ? <Empty icon="bag" title="Open your store first" text="Choose Go to store settings, check the name buyers will see, then choose Open my store. If the wallet is locked, unlock it so the node can sign the store. New listing turns on after that."><button className="ob-primary" onClick={onOpenSettings}>Go to store settings</button></Empty> : !listings.length ? <Empty icon="bag" title="No listings yet" text="Publish a listing and it shows on your store"/> : !visible.length ? <p className="ob-shipping-note">No listings match these filters <button className="ob-text-link" onClick={() => { setQuery(""); setStockFilter("all"); }}>Clear filters</button></p> : <ul className="ob-own-listings">{visible.map(listing => {
      const estimate = displayEstimate(listing.priceSats, rates, currency, listing.price?.currency);
      const stock = listing.stock > 0 ? `${listing.stock} in stock` : "Out of stock";
      return <li key={listing.hash} className={`ob-own-listing${listing.listingId ? " is-selectable" : ""}`}>
      {listing.listingId ? <label className="ob-default-address"><input type="checkbox" checked={selected.includes(listing.listingId)} onChange={() => toggle(listing.listingId!)} aria-label={`Select ${listing.title}`}/></label> : null}
      <span className="ob-own-listing-image">{listing.images[0] ? <MediaImage hash={listing.images[0]} alt={listing.title}/> : <span aria-hidden="true">{listing.title.charAt(0)}</span>}</span>
      <span className="ob-own-listing-copy">
        <strong>{listing.title}</strong>
        <span className="ob-own-listing-meta">
          <span>{listing.category}</span>
          {listing.kind === "digital" ? <span>Digital</span> : listing.shipFrom ? <span className="ob-own-listing-from">Ships from {listing.shipFrom}</span> : null}
          <span className="ob-own-listing-stock">{stock}</span>
          {listing.contentRating !== "general" ? <span>{listing.contentRating}</span> : null}
        </span>
      </span>
      <span className="ob-own-listing-price">
        <span className="ob-own-listing-sats">{listing.priceSats.toLocaleString("en-US")} <small>sats</small></span>
        {listing.price && listing.price.currency !== "SAT" ? <span className="ob-own-listing-ask">{formatPrice(listing.price.amount, listing.price.currency)}</span> : null}
        {estimate ? <span className="ob-own-listing-estimate">≈ {estimate}</span> : null}
      </span>
      <span className="ob-own-listing-actions">
        {confirm === listing.hash ? <>
          <button className="ob-secondary" onClick={() => setConfirm(null)}>Keep</button>
          <button className="ob-danger" onClick={() => void remove(listing)}>Remove listing</button>
        </> : <>
          <button className="ob-secondary" onClick={() => setEditing(listing)} aria-label={`Edit ${listing.title}`}>Edit</button>
          <button className="ob-text-link" onClick={() => setConfirm(listing.hash)} aria-label={`Remove ${listing.title}`}>Remove</button>
        </>}
      </span>
    </li>;
    })}</ul>}
    {(drafts.data ?? []).length > 0 && <ul className="ob-own-listings">{(drafts.data ?? []).map(draft => <li key={draft.draftId} className="ob-own-listing is-draft">
      <span className="ob-own-listing-copy">
        <strong>{draft.listing.title || "Untitled draft"}</strong>
        <span>Draft, not published</span>
      </span>
      <span className="ob-own-listing-actions">
        <button className="ob-secondary" onClick={() => setEditing(draft)} aria-label={`Edit draft ${draft.listing.title || "listing"}`}>Edit draft</button>
      </span>
    </li>)}</ul>}
    {dialog}
  </section>;
}
