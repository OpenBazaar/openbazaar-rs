import { useState } from "react";
import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import { deleteListing, fetchDrafts, fetchOwnListings, formatPrice, type SavedDraft, type SellerProfile } from "../seller";
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
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const listings = resource.data ?? [];
  async function changed(message: string) {
    setEditing(null);
    await resource.refresh();
    await drafts.refresh();
    onChanged(message);
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
    {resource.loading ? <p role="status">Loading your listings…</p> : !listings.length ? <Empty icon="bag" title="No listings yet" text="Add your first listing with photos, a price, and how many you have."/> : <ul className="ob-own-listings">{listings.map(listing => <li key={listing.hash} className="ob-own-listing">
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
