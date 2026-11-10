import assert from "node:assert/strict";
import { test } from "node:test";

import { canShrink, hasAlpha, shrinkImage, shrinkSizes } from "./imageShrink.ts";

const CAP = 2 * 1024 * 1024;

test("the sizes to try keep the shape, step down, and never enlarge", () => {
  assert.deepEqual(shrinkSizes(4000, 3000), [
    { width: 2000, height: 1500 },
    { width: 1600, height: 1200 },
    { width: 1200, height: 900 },
    { width: 800, height: 600 },
  ]);
  assert.deepEqual(shrinkSizes(3000, 4000)[0], { width: 1500, height: 2000 });
  assert.deepEqual(shrinkSizes(1500, 1000), [
    { width: 1500, height: 1000 },
    { width: 1200, height: 800 },
    { width: 800, height: 533 },
  ]);
  assert.deepEqual(shrinkSizes(640, 480), [{ width: 640, height: 480 }]);
  assert.deepEqual(shrinkSizes(8000, 2)[0], { width: 2000, height: 1 });
});

test("transparency is any pixel that is not fully opaque", () => {
  assert.equal(hasAlpha([0, 0, 0, 255, 9, 9, 9, 255]), false);
  assert.equal(hasAlpha([0, 0, 0, 255, 9, 9, 9, 254]), true);
  assert.equal(hasAlpha([]), false);
});

test("there is nothing to shrink with outside a browser", () => {
  assert.equal(canShrink(), false);
});

/// A canvas whose encoded size is `bytesPerPixel` of what was drawn, and a decoder that answers `source`.
function fakeBrowser(source: { width: number; height: number } | null, bytesPerPixel: number, alpha = false) {
  const globals = globalThis as { document?: unknown; createImageBitmap?: unknown };
  const canvas = {
    width: 0,
    height: 0,
    types: [] as string[],
    getContext: () => ({ drawImage() {}, getImageData: () => ({ data: [0, 0, 0, alpha ? 0 : 255] }) }),
    toBlob(done: (blob: Blob) => void, type: string) {
      canvas.types.push(type);
      done(new Blob([new Uint8Array(Math.ceil(canvas.width * canvas.height * bytesPerPixel))], { type }));
    },
  };
  globals.document = { createElement: () => canvas };
  globals.createImageBitmap = async (_file: Blob, options: { imageOrientation?: string }) => {
    assert.equal(options.imageOrientation, "from-image");
    if (!source) throw new Error("not an image");
    return { ...source, close() {} };
  };
  return {
    canvas,
    restore() {
      delete globals.document;
      delete globals.createImageBitmap;
    },
  };
}

test("a large photo becomes a JPEG at the first size that fits with room to spare", async () => {
  const photo = new Blob([new Uint8Array(8)], { type: "image/jpeg" });
  let browser = fakeBrowser({ width: 4000, height: 3000 }, 0.5);
  try {
    assert.equal(canShrink(), true);
    const shrunk = await shrinkImage(photo, CAP);
    assert.deepEqual([shrunk?.type, shrunk?.size, browser.canvas.width, browser.canvas.height], ["image/jpeg", 1_500_000, 2000, 1500]);
  } finally {
    browser.restore();
  }
  // 1600 × 1200 at one byte a pixel is under the cap but not under nine tenths of it.
  browser = fakeBrowser({ width: 4000, height: 3000 }, 1);
  try {
    const shrunk = await shrinkImage(photo, CAP);
    assert.deepEqual([shrunk?.size, browser.canvas.width, browser.canvas.height], [1_080_000, 1200, 900]);
    assert.deepEqual(browser.canvas.types, ["image/jpeg", "image/jpeg", "image/jpeg"]);
  } finally {
    browser.restore();
  }
});

test("an image with transparency stays a PNG, and a JPEG is never checked for it", async () => {
  const browser = fakeBrowser({ width: 3000, height: 3000 }, 0.4, true);
  try {
    assert.equal((await shrinkImage(new Blob([], { type: "image/png" }), CAP))?.type, "image/png");
    assert.equal((await shrinkImage(new Blob([], { type: "image/webp" }), CAP))?.type, "image/png");
    assert.equal((await shrinkImage(new Blob([], { type: "image/jpeg" }), CAP))?.type, "image/jpeg");
  } finally {
    browser.restore();
  }
});

test("a GIF, an image that does not decode, and one too large at the smallest size are not shrunk", async () => {
  let browser = fakeBrowser({ width: 4000, height: 3000 }, 0.1);
  try {
    assert.equal(await shrinkImage(new Blob([], { type: "image/gif" }), CAP), null);
  } finally {
    browser.restore();
  }
  browser = fakeBrowser(null, 0.1);
  try {
    assert.equal(await shrinkImage(new Blob([], { type: "image/png" }), CAP), null);
  } finally {
    browser.restore();
  }
  browser = fakeBrowser({ width: 4000, height: 3000 }, 5);
  try {
    assert.equal(await shrinkImage(new Blob([], { type: "image/png" }), CAP), null);
    assert.deepEqual([browser.canvas.width, browser.canvas.height], [800, 600]);
  } finally {
    browser.restore();
  }
});
