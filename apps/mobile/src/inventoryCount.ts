/** The count beside the inventory filters: published listings shown, then saved drafts. */
export function inventoryCount(published: number, shown: number, drafts: number): string {
  const listed = shown === published ? `${published} published` : `${shown} of ${published} published`;
  return drafts ? `${listed} · ${drafts} ${drafts === 1 ? "draft" : "drafts"}` : listed;
}
