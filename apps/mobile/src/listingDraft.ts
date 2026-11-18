/** Where an unpublished listing is saved. Publishing uses `/store/listings` instead. */
export function draftPath(id: string | null): string {
  return id ? `/store/drafts/${id}` : "/store/drafts";
}

/** The published listing a draft edits, so publishing it updates that listing instead of adding one. */
export function draftListingId(listing: { listingId: string | null } | null, saved: { listing: { listingId?: string | null } } | null | undefined): string | null {
  return listing?.listingId ?? saved?.listing.listingId ?? null;
}
