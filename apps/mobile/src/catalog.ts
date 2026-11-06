import raw from "./catalog.json";

export type ListingKind = "physical" | "digital";

export type ListingHeader = {
  hash: string;
  storeId: string;
  title: string;
  category: string;
  priceSats: number;
  shipFrom: string;
  kind: ListingKind;
};

export type ShopCatalog = {
  store: {
    id: string;
    name: string;
    region: string;
  };
  listings: ListingHeader[];
};

function isListingKind(value: string): value is ListingKind {
  return value === "physical" || value === "digital";
}

function loadCatalog(value: typeof raw): ShopCatalog {
  return {
    store: value.store,
    listings: value.listings.map((listing) => {
      if (!isListingKind(listing.kind)) {
        throw new Error(`Unknown listing kind: ${listing.kind}`);
      }
      return { ...listing, kind: listing.kind };
    }),
  };
}

export const catalog = loadCatalog(raw);
