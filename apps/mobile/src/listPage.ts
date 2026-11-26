/** Bounded page of a filtered list. Empty lists report one empty page. */
export function listPage<T>(items: readonly T[], requestedPage: number, size: number) {
  const pages = Math.max(1, Math.ceil(items.length / size));
  const page = Math.max(0, Math.min(requestedPage, pages - 1));
  const start = page * size;
  return { items: items.slice(start, start + size), page, pages, start, size };
}
