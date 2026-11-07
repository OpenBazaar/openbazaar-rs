// Demo photos shipped in the app. A listing with no signed image hash can
// still show these. Real photos come from the node by hash, not from these files.
const PHOTOS: Record<string, number[]> = {
  "Paperback novel": [require("../../public/goods/paperback.jpg"), require("../../public/goods/paperback-open.jpg")],
  "Field guide to tide pools": [require("../../public/goods/paperback.jpg")],
  "Whole bean coffee, 12 oz": [require("../../public/goods/coffee.jpg")],
  "Wildflower honey, 8 oz": [require("../../public/goods/honey.jpg")],
  "Linen dish towel": [require("../../public/goods/towel.jpg")],
  "Stoneware mug": [require("../../public/goods/mug.jpg")],
  "Wool socks": [require("../../public/goods/socks.jpg")],
  "Clothbound notebook": [require("../../public/goods/notebook.jpg")],
  Sketchbook: [require("../../public/goods/notebook.jpg")],
  "Beeswax candles, set of 4": [require("../../public/goods/candles.jpg")],
  "Work shirt": [require("../../public/goods/shirt.jpg")],
  "Cotton apron": [require("../../public/goods/shirt.jpg")],
  "Garden trowel": [require("../../public/goods/trowel.jpg")],
  "Letterpress cards": [require("../../public/goods/cards.jpg")],
};

export function bundledPhoto(title: string, index = 0): number | undefined {
  return PHOTOS[title]?.[index];
}

export function bundledPhotoCount(title: string): number {
  return PHOTOS[title]?.length ?? 0;
}
