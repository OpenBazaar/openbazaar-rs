/** Where an unpublished listing is saved. Publishing uses `/store/listings` instead. */
export function draftPath(id: string | null): string {
  return id ? `/store/drafts/${id}` : "/store/drafts";
}
