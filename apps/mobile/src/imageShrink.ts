// Shrinks an image that is over the node's upload cap so it fits, in the
// browser, with a canvas. The native app has no canvas and does not shrink.

/** Longest side in pixels, tried in this order until the result fits. */
export const SHRINK_SIDES: readonly number[] = [2000, 1600, 1200, 800];
/** JPEG quality at every step. PNG output has no quality setting. */
export const JPEG_QUALITY = 0.85;
/** The result is kept only if it is at most this share of the cap. */
export const FIT_SHARE = 0.9;

export type Size = { width: number; height: number };

/// The sizes to try for an image, largest first: the same shape, never larger
/// than the image, and each size once.
export function shrinkSizes(width: number, height: number): Size[] {
  const sizes: Size[] = [];
  for (const side of SHRINK_SIDES) {
    const scale = Math.min(1, side / Math.max(width, height));
    const size = { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
    const last = sizes[sizes.length - 1];
    if (!last || last.width !== size.width || last.height !== size.height) sizes.push(size);
  }
  return sizes;
}

/// Whether any pixel of RGBA data is not fully opaque.
export function hasAlpha(rgba: ArrayLike<number>): boolean {
  for (let i = 3; i < rgba.length; i += 4) if (rgba[i] < 255) return true;
  return false;
}

/// Whether this runtime can shrink images at all. The native app cannot.
export function canShrink(): boolean {
  return typeof document !== "undefined" && typeof createImageBitmap === "function";
}

/// `file` drawn smaller until it is under `maxBytes`, or null when it cannot
/// be: a GIF (a canvas keeps only the first frame of an animation), an image
/// this browser cannot decode, or one still too large at the smallest size.
/// The result is upright, a JPEG, or a PNG when the image has transparency.
export async function shrinkImage(file: Blob, maxBytes: number): Promise<Blob | null> {
  if (file.type === "image/gif") return null;
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
  } catch {
    return null;
  }
  try {
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d");
    if (!context) return null;
    let type = "";
    for (const { width, height } of shrinkSizes(bitmap.width, bitmap.height)) {
      // Setting the size clears the canvas and its settings.
      canvas.width = width;
      canvas.height = height;
      context.imageSmoothingQuality = "high";
      context.drawImage(bitmap, 0, 0, width, height);
      // A JPEG would paint transparent pixels black.
      type ||= file.type !== "image/jpeg" && hasAlpha(context.getImageData(0, 0, width, height).data) ? "image/png" : "image/jpeg";
      const blob = await new Promise<Blob | null>((done) => canvas.toBlob(done, type, JPEG_QUALITY));
      if (blob && blob.size <= maxBytes * FIT_SHARE) return blob;
    }
    return null;
  } finally {
    bitmap.close();
  }
}
