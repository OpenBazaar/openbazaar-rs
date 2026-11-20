// The fixed choices the listing editor offers in place of free text, and how
// they map onto the fields a listing already signs. Nothing here changes a
// record's format: a category is the taxonomy number of a version 7 listing
// plus the category text every version carries, and ships-from is still the
// short region text, now always a country name.

import type { Commerce } from "./commerce.ts";
import { TAXONOMY } from "./commerceDraft.ts";
import { displayText } from "./safeText.ts";

export type CategoryFields = { taxonomy: string; subcategory: string };

const MAX_CATEGORY = 40;

function bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

function byId(id: number) {
  return TAXONOMY.find(entry => entry.id === id) ?? null;
}

function byName(name: string) {
  const wanted = name.trim().toLowerCase();
  return wanted ? TAXONOMY.find(entry => entry.name.toLowerCase() === wanted) ?? null : null;
}

/// The category and subcategory to show for a listing. A listing signed
/// before categories were fixed has only its own text: that text becomes the
/// subcategory, and the seller picks a category the next time they save.
export function categoryFieldsOf(category: string, taxonomy: number): CategoryFields {
  const top = byId(taxonomy);
  const named = byName(category);
  const legacy = byId(LEGACY.get(category.trim().toLowerCase()) ?? 0);
  if (top) return { taxonomy: String(top.id), subcategory: named?.id === top.id || legacy?.id === top.id ? "" : category };
  if (named) return { taxonomy: String(named.id), subcategory: "" };
  if (legacy) return { taxonomy: String(legacy.id), subcategory: "" };
  return { taxonomy: "", subcategory: category };
}

/// The category text and taxonomy number the listing signs, or what to fix.
/// A sats-priced listing is signed without the version 7 block, so it carries
/// the category by name alone and has no room for a subcategory.
export function signedCategory(fields: CategoryFields, fiat: boolean): { category: string; taxonomy: number } | string {
  const top = /^\d+$/.test(fields.taxonomy.trim()) ? byId(Number(fields.taxonomy)) : null;
  if (!top) return "Choose a category.";
  const subcategory = fields.subcategory.trim();
  if (bytes(subcategory) > MAX_CATEGORY) return `A subcategory is at most ${MAX_CATEGORY} characters.`;
  return fiat ? { category: subcategory || top.name, taxonomy: top.id } : { category: top.name, taxonomy: 0 };
}

// Names the sample catalog and early listings used before categories were fixed.
const LEGACY: ReadonlyMap<string, number> = new Map([["home", 5], ["grocery", 13], ["music", 10], ["clothing", 3], ["tools", 11], ["paper", 11]]);

const OTHER = "Other";

/// The category a buyer browses a listing under: its fixed category when it
/// signs one, by number or by name, else "Other". Never the listing's own
/// text, so a seller cannot add entries to the buyer's navigation.
export function topCategory(listing: { category: string; commerce?: Pick<Commerce, "taxonomy"> | null }): string {
  const legacy = LEGACY.get(listing.category.trim().toLowerCase());
  return (byId(listing.commerce?.taxonomy ?? 0) ?? byName(listing.category) ?? (legacy ? byId(legacy) : null))?.name ?? OTHER;
}

/// The fixed categories in the order the buyer's navigation lists them.
export const CATEGORY_NAMES: readonly string[] = TAXONOMY.map(entry => entry.name);

/// The seller's own category text, cleaned for display on a card or listing page.
export function categoryLabel(listing: { category: string }): string {
  return displayText(listing.category, MAX_CATEGORY) || OTHER;
}

/// Shipping service names offered before "Other".
export const SERVICE_NAMES: readonly string[] = ["Standard", "Economy", "Expedited", "Express", "Overnight", "Tracked", "Freight", "Local delivery"];

/// Option names offered before "Other".
export const OPTION_NAMES: readonly string[] = ["Size", "Color", "Material", "Style", "Finish", "Scent", "Flavor", "Length", "Pack size"];

/// Item specifics offered before "Other". The product codes are checked when the listing is saved.
export const SPECIFIC_KEYS: readonly string[] = ["Brand", "Model", "Material", "Color", "Size", "Dimensions", "Weight", "Year", "Author", "Publisher", "Format", "Language", "Country of origin", "MPN", "UPC", "EAN", "ISBN", "GTIN"];

/// True when `value` is the choice `option`, however it was typed or stored
/// ("Country of origin" is stored as "country-of-origin").
export function sameChoice(option: string, value: string): boolean {
  const norm = (text: string) => text.trim().toLowerCase().replace(/[\s_]+/g, "-");
  return norm(option) === norm(value);
}

/// Volume tier rows to show: blank rows are dropped, so a listing without tiers shows none.
export function filledTiers<T extends { qty: string; price: string }>(rows: T[]): T[] {
  return rows.filter(row => row.qty.trim() || row.price.trim());
}

/// Specific rows to show: blank rows are dropped.
export function filledSpecifics<T extends { key: string; value: string }>(rows: T[]): T[] {
  return rows.filter(row => row.key.trim() || row.value.trim());
}
