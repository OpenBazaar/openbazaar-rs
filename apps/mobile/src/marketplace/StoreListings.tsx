import { useState } from "react";
import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import { purchaseTerms, type AvailabilityDraft } from "../commerceDraft";
import { bulkEditListings, deleteListing, fetchDrafts, fetchOwnListings, formatPrice, type SavedDraft, type SellerProfile } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { ListingEditor } from "./ListingEditor";
import { Empty, Money } from "./Marketplace";
import { PageHeading } from "./PageHeading";
import { MediaImage, useLockedAction } from "./SellerKit";

/** The seller's own listings: add, edit, and remove. Each change is signed by the node. */
export function StoreListings({ profile, rates, currency, startNew = false, onChanged }: { profile: SellerProfile; rates: RateTable; currency: CurrencyCode; startNew?: boolean; onChanged: (message: string) => void }) {
  const resource = useNodeResource(() => fetchOwnListings(profile.storeKey), 0);
  const drafts = useNodeResource(() => fetchDrafts(), 0);
  const [editing, setEditing] = useState<ListingHeader | "new" | SavedDraft | null>(startNew && profile.setUp ? "new" : null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [bulk, setBulk] = useState<"price" | "availability">("price");
  const [bulkPrice, setBulkPrice] = useState("");
  const [bulkAvailability, setBulkAvailability] = useState<AvailabilityDraft>({ kind: "in-stock", shipsOn: "", days: "" });
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const listings = resource.data ?? [];
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
      if (!/^\d+$/.test(bulkPrice.trim()) || Number(bulkPrice) === 0) {
        setError(bulkPrice.trim() === "0" ? "Enter a price above zero." : "The price must be a whole number of the smallest unit.");
        return;
      }
      change = { price: Number(bulkPrice) };
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
    <PageHeading title={editing === "new" ? "New listing" : savedDraft ? `Draft ${savedDraft.listing.title || "listing"}` : `Edit ${published?.title ?? "listing"}`} description="Save a draft to keep it on this node. Publish signs it and serves it to buyers."/>
    <ListingEditor listing={published} savedDraft={savedDraft} categories={[...new Set(listings.map(l => l.category))]} rates={rates} onSaved={message => void changed(message)} onCancel={() => setEditing(null)}/>
  </section>;
  return <section className="ob-store-listings">
    <PageHeading title="Your listings" description={profile.setUp ? `What ${profile.name} sells. Buyers see each listing as your node signed it.` : "Set up your store first, then add what you sell."} action={<button className="ob-primary" disabled={!profile.setUp} onClick={() => setEditing("new")}>New listing</button>}/>
    {(error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    {!!listings.length && <div className="ob-store-section">
      <p className="ob-shipping-note">Apply one change to the listings you select. A draft stays a draft.</p>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Change</span><select value={bulk} onChange={e => setBulk(e.target.value as "price" | "availability")}><option value="price">Regular price</option><option value="availability">Availability</option></select></label>
        {bulk === "price" ? <label className="ob-field"><span>Price</span><input inputMode="numeric" value={bulkPrice} placeholder="1800" onChange={e => setBulkPrice(e.target.value)}/><small className="ob-field-hint">Smallest unit of each listing's currency.</small></label> : <>
          <label className="ob-field"><span>Availability</span><select value={bulkAvailability.kind} onChange={e => setBulkAvailability({ ...bulkAvailability, kind: e.target.value })}><option value="in-stock">In stock</option><option value="pre-order">Pre-order</option><option value="made-to-order">Made to order</option><option value="away">Away</option></select></label>
          {bulkAvailability.kind === "pre-order" && <label className="ob-field"><span>Ships on</span><input type="date" value={bulkAvailability.shipsOn} onChange={e => setBulkAvailability({ ...bulkAvailability, shipsOn: e.target.value })}/></label>}
          {bulkAvailability.kind === "made-to-order" && <label className="ob-field"><span>Ready in (days)</span><input inputMode="numeric" value={bulkAvailability.days} placeholder="14" onChange={e => setBulkAvailability({ ...bulkAvailability, days: e.target.value })}/></label>}
        </>}
      </div>
      <button className="ob-primary" disabled={!selected.length} onClick={() => void applyBulk()}>Apply to selected</button>
    </div>}
    {resource.loading ? <p role="status">Loading your listings…</p> : !listings.length ? <Empty icon="bag" title="No listings yet" text="Add your first listing with photos, a price, and how many you have."/> : <ul className="ob-own-listings">{listings.map(listing => <li key={listing.hash} className="ob-own-listing">
      {listing.listingId ? <label className="ob-default-address"><input type="checkbox" checked={selected.includes(listing.listingId)} onChange={() => toggle(listing.listingId!)} aria-label={`Select ${listing.title}`}/></label> : null}
      <span className="ob-own-listing-image">{listing.images[0] ? <MediaImage hash={listing.images[0]} alt={listing.title}/> : <span aria-hidden="true">{listing.title.charAt(0)}</span>}</span>
      <span className="ob-own-listing-copy">
        <strong>{listing.title}</strong>
        <span>{listing.category} · {listing.kind === "digital" ? "Digital" : `Ships from ${listing.shipFrom}`} · {listing.stock > 0 ? `${listing.stock} in stock` : "Out of stock"}{listing.contentRating !== "general" ? ` · ${listing.contentRating}` : ""}</span>
      </span>
      <span className="ob-own-listing-price">{listing.price ? <strong>{formatPrice(listing.price.amount, listing.price.currency)}</strong> : null}<Money sats={listing.priceSats} rates={rates} currency={currency}/></span>
      <span className="ob-own-listing-actions">
        {confirm === listing.hash ? <>
          <button className="ob-secondary" onClick={() => setConfirm(null)}>Keep</button>
          <button className="ob-danger" onClick={() => void remove(listing)}>Remove listing</button>
        </> : <>
          <button className="ob-secondary" onClick={() => setEditing(listing)} aria-label={`Edit ${listing.title}`}>Edit</button>
          <button className="ob-text-link" onClick={() => setConfirm(listing.hash)} aria-label={`Remove ${listing.title}`}>Remove</button>
        </>}
      </span>
    </li>)}</ul>}
    {(drafts.data ?? []).length > 0 && <ul className="ob-own-listings">{(drafts.data ?? []).map(draft => <li key={draft.draftId} className="ob-own-listing">
      <span className="ob-own-listing-copy">
        <strong>{draft.listing.title || "Untitled draft"}</strong>
        <span>Draft · not published</span>
      </span>
      <span className="ob-own-listing-actions">
        <button className="ob-secondary" onClick={() => setEditing(draft)} aria-label={`Edit draft ${draft.listing.title || "listing"}`}>Edit draft</button>
      </span>
    </li>)}</ul>}
    {dialog}
  </section>;
}
