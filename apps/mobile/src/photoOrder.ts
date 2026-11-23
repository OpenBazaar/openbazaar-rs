/** Move the photo at `index` earlier or later. The first photo stays the cover. */
export function movePhoto(images: readonly string[], index: number, direction: -1 | 1): string[] {
  const next = index + direction;
  if (index < 0 || next < 0 || index >= images.length || next >= images.length) return [...images];
  const copy = [...images];
  const [photo] = copy.splice(index, 1);
  copy.splice(next, 0, photo);
  return copy;
}
