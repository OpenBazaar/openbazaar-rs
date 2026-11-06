const PHOTOS: Record<string, readonly string[]> = {
  "Paperback novel": ["goods/paperback.jpg", "goods/paperback-open.jpg"],
  "Field guide to tide pools": ["goods/paperback.jpg"],
  "Whole bean coffee, 12 oz": ["goods/coffee.jpg"],
  "Wildflower honey, 8 oz": ["goods/honey.jpg"],
  "Linen dish towel": ["goods/towel.jpg"],
  "Stoneware mug": ["goods/mug.jpg"],
  "Wool socks": ["goods/socks.jpg"],
  "Clothbound notebook": ["goods/notebook.jpg"],
  Sketchbook: ["goods/notebook.jpg"],
  "Beeswax candles, set of 4": ["goods/candles.jpg"],
  "Work shirt": ["goods/shirt.jpg"],
  "Cotton apron": ["goods/shirt.jpg"],
  "Garden trowel": ["goods/trowel.jpg"],
  "Letterpress cards": ["goods/cards.jpg"],
};

export function photosFor(title: string, category: string): string[] {
  return [...(PHOTOS[title] ?? [`covers/${category.toLowerCase()}.svg`])];
}
