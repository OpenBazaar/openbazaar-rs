// The seller's editor model for a listing's options and terms.
//
// The editor works on the same Commerce shape the node returns and accepts.
// Variants are generated from the option axes: every combination gets a row,
// and a row the seller switches off is left out of the signed listing (it is
// "not offered"). Existing rows keep their id, so orders placed against an
// earlier version still find their variant after an edit.

import type { Axis, Commerce, Returns, Sale, Service, Variant } from "./commerce.ts";
import { fromMinor, toMinor } from "./pricing.ts";

export const MAX_AXES = 3;
export const MAX_VALUES = 16;
export const MAX_VARIANTS = 100;

/// A grid row: a variant, and whether the seller offers it.
export type VariantRow = Variant & { offered: boolean };

export function emptyCommerce(): Commerce {
  return {
    condition: "new", conditionNote: "", taxonomy: 0, specifics: {}, axes: [], variants: [], compareAt: 0, sale: null, tiers: [],
    acceptsOffers: false, maxPerOrder: 0, lotSize: 1, availability: { kind: "in-stock" },
    shipping: { weightG: 0, dimsMm: [0, 0, 0], handlingDays: 1, freeOver: 0, services: [], pickup: null },
    returns: { kind: "store" },
  };
}

export function defaultService(): Service {
  return { name: "Standard", price: 0, extraItem: 0, daysMin: 3, daysMax: 7, regions: [] };
}

const keyOf = (options: string[]) => options.map(o => o.toLowerCase()).join("\u0000");

/// Every combination of the axes' values, in axis order.
export function combinations(axes: Axis[]): string[][] {
  return axes.reduce<string[][]>((rows, axis) => rows.flatMap(row => axis.values.map(value => [...row, value])), [[]]).filter(row => row.length === axes.length && row.length > 0);
}

/// The grid for `axes`, reusing `rows` that name the same options.
export function syncRows(axes: Axis[], rows: VariantRow[]): VariantRow[] {
  const byKey = new Map(rows.map(row => [keyOf(row.options), row]));
  return combinations(axes).map(options => {
    const existing = byKey.get(keyOf(options));
    return existing ? { ...existing, options } : { id: "", options, sku: "", price: 0, stock: 0, image: 0, weightG: 0, offered: true };
  });
}

export function rowsOf(commerce: Commerce): VariantRow[] {
  return syncRows(commerce.axes, commerce.variants.map(v => ({ ...v, offered: true }))).map(row => ({
    ...row,
    offered: commerce.variants.some(v => keyOf(v.options) === keyOf(row.options)),
  }));
}

/// Split "S, M, L" into trimmed values, dropping blanks and repeats.
export function parseValues(text: string): string[] {
  const seen = new Set<string>();
  return text.split(",").map(v => v.trim()).filter(v => v && !seen.has(v.toLowerCase()) && seen.add(v.toLowerCase()));
}

/// What to fix before saving, or null.
export function axesProblem(axes: Axis[], rows: VariantRow[]): string | null {
  if (axes.length > MAX_AXES) return `A listing has at most ${MAX_AXES} options.`;
  const names = new Set<string>();
  for (const axis of axes) {
    if (!axis.name.trim()) return "Name each option, like Size or Color.";
    if (names.has(axis.name.trim().toLowerCase())) return `"${axis.name}" is used twice.`;
    names.add(axis.name.trim().toLowerCase());
    if (!axis.values.length) return `Give ${axis.name} at least one value.`;
    if (axis.values.length > MAX_VALUES) return `${axis.name} has more than ${MAX_VALUES} values.`;
  }
  const offered = rows.filter(r => r.offered);
  if (axes.length && !offered.length) return "Offer at least one combination.";
  if (offered.length > MAX_VARIANTS) return `A listing has at most ${MAX_VARIANTS} variants; switch some combinations off.`;
  return null;
}

/// The commerce object the node signs: offered rows only, without the
/// editor's bookkeeping, and new rows without an id.
export function withRows(commerce: Commerce, axes: Axis[], rows: VariantRow[]): Commerce {
  const variants = axes.length ? rows.filter(r => r.offered).map(({ offered: _, ...variant }) => variant) : [];
  return { ...commerce, axes: axes.length ? axes : [], variants };
}

export type SaleFields = { price: string; startsOn: string; endsOn: string };

const BLANK_SALE: SaleFields = { price: "", startsOn: "", endsOn: "" };

function discountOf(amount: number, bps: number): number {
  return Number((BigInt(amount) * BigInt(bps)) / 10_000n);
}

function dayNumber(text: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (year < 2000 || month < 1 || month > 12 || day < 1 || day > 31) return null;
  return year * 10_000 + month * 100 + day;
}

function isoDay(day: number): string {
  const text = String(day).padStart(8, "0");
  return `${text.slice(0, 4)}-${text.slice(4, 6)}-${text.slice(6, 8)}`;
}

/// The sale price and dates to show for a sale already on the listing.
export function saleFieldsOf(regular: number, sale: Sale | null, currency: string): SaleFields {
  if (!sale || regular <= 0) return BLANK_SALE;
  return { price: fromMinor(regular - discountOf(regular, sale.offBps), currency), startsOn: isoDay(sale.startsOn), endsOn: isoDay(sale.endsOn) };
}

/// The sale the listing signs, or a reason to refuse it. Blank fields mean no sale.
/// The price is the amount buyers pay; the record stores that discount and these days.
export function scheduledSale(regular: number, currency: string, fields: SaleFields): Sale | null | string {
  const price = fields.price.trim();
  const starts = fields.startsOn.trim();
  const ends = fields.endsOn.trim();
  if (!price && !starts && !ends) return null;
  const amount = price ? toMinor(price, currency) : null;
  if (amount == null || amount <= 0) return "Enter a sale price.";
  if (amount >= regular) return "A sale price is below the regular price.";
  if (!starts || !ends) return "A sale needs a start and an end.";
  const startsOn = dayNumber(starts);
  const endsOn = dayNumber(ends);
  if (startsOn == null || endsOn == null) return "A sale date is written as a day.";
  if (endsOn < startsOn) return "A sale ends before it starts.";
  const discount = regular - amount;
  let offBps: number | null = null;
  for (let bps = 1; bps <= 9_000; bps++) {
    if (discountOf(regular, bps) === discount) {
      offBps = bps;
      break;
    }
  }
  if (offBps == null) return "A sale takes 0.01% to 90% off.";
  return { offBps, startsOn, endsOn };
}

/// Put `sale` on the commerce block the existing sign path already accepts.
export function withSale(commerce: Commerce | null, sale: Sale | null): Commerce | null {
  if (!commerce && !sale) return null;
  return { ...(commerce ?? emptyCommerce()), sale };
}

export type PackageInput = {
  weightG: string;
  lengthMm: string;
  widthMm: string;
  heightMm: string;
  handlingDays: string;
  freeOver: string;
  /** One ships-to line per shipping service. Empty means worldwide. */
  regions: string[];
};

export type PackageTerms = {
  weightG: number;
  dimsMm: [number, number, number];
  handlingDays: number;
  freeOver: number;
  regions: string[][];
};

const BLANK_PACKAGE: Omit<PackageInput, "regions"> = { weightG: "", lengthMm: "", widthMm: "", heightMm: "", handlingDays: "", freeOver: "" };

function whole(text: string, max: number, label: string): number | string {
  const trimmed = text.trim();
  if (!trimmed) return 0;
  if (!/^\d+$/.test(trimmed)) return label;
  const value = Number(trimmed);
  if (!Number.isSafeInteger(value) || value > max) return label;
  return value;
}

/// Country codes the way the signed service stores them: uppercase, unique, sorted.
export function regionList(text: string): string[] | string {
  const codes = text.split(/[\s,]+/).map(code => code.trim().toUpperCase()).filter(Boolean);
  if (codes.length > 32) return "A service ships to at most 32 countries.";
  if (codes.some(code => !/^[A-Z]{2}$/.test(code))) return "A region is a two-letter country code.";
  return [...new Set(codes)].sort();
}

/// Package fields to show for shipping already on the listing.
export function packageFieldsOf(shipping: Commerce["shipping"] | null | undefined, currency: string): Omit<PackageInput, "regions"> {
  if (!shipping) return BLANK_PACKAGE;
  const [lengthMm, widthMm, heightMm] = shipping.dimsMm;
  return {
    weightG: shipping.weightG ? String(shipping.weightG) : "",
    lengthMm: lengthMm ? String(lengthMm) : "",
    widthMm: widthMm ? String(widthMm) : "",
    heightMm: heightMm ? String(heightMm) : "",
    handlingDays: shipping.handlingDays ? String(shipping.handlingDays) : "",
    freeOver: shipping.freeOver ? fromMinor(shipping.freeOver, currency) : "",
  };
}

/// The package, handling time, free-shipping threshold and ships-to list the listing signs.
export function shippingTerms(currency: string, input: PackageInput): PackageTerms | string {
  const weightG = whole(input.weightG, 0xffff_ffff, "Package weight is a whole number of grams.");
  if (typeof weightG === "string") return weightG;
  const lengthMm = whole(input.lengthMm, 65_535, "A package side is at most 65535 mm.");
  if (typeof lengthMm === "string") return lengthMm;
  const widthMm = whole(input.widthMm, 65_535, "A package side is at most 65535 mm.");
  if (typeof widthMm === "string") return widthMm;
  const heightMm = whole(input.heightMm, 65_535, "A package side is at most 65535 mm.");
  if (typeof heightMm === "string") return heightMm;
  const handlingDays = whole(input.handlingDays, 30, "Handling time is at most 30 days.");
  if (typeof handlingDays === "string") return handlingDays;
  const freeText = input.freeOver.trim();
  const freeOver = freeText ? toMinor(freeText, currency) : 0;
  if (freeOver == null) return "Enter the free-shipping amount, or leave it empty.";
  const regions: string[][] = [];
  for (const text of input.regions) {
    const list = regionList(text);
    if (typeof list === "string") return list;
    regions.push(list);
  }
  return { weightG, dimsMm: [lengthMm, widthMm, heightMm], handlingDays, freeOver, regions };
}

/// Put the package on the commerce block the existing sign path already accepts.
export function withPackage(commerce: Commerce | null, terms: PackageTerms): Commerce | null {
  const bare = terms.weightG === 0 && terms.handlingDays === 0 && terms.freeOver === 0 && terms.dimsMm.every(side => side === 0) && terms.regions.every(list => list.length === 0);
  if (!commerce && bare) return null;
  const base = commerce ?? emptyCommerce();
  const services = base.shipping.services.map((service, index) => ({ ...service, regions: terms.regions[index] ?? [] }));
  return { ...base, shipping: { ...base.shipping, weightG: terms.weightG, dimsMm: terms.dimsMm, handlingDays: terms.handlingDays, freeOver: terms.freeOver, services } };
}

export type SpecificRow = { key: string; value: string };
export type ReturnsDraft = { kind: "store" | "none" | "days"; days: string; sellerPays: boolean };
export type ListingFacts = { specifics: Record<string, string>; returns: Returns };

const CODE_LENGTHS: Record<string, readonly number[]> = { upc: [12], ean: [8, 13], isbn: [13], gtin: [8, 12, 13, 14] };

function bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

function specificKey(text: string): string {
  return text.trim().toLowerCase().replaceAll(" ", "-").replaceAll("_", "-");
}

function gtinCheck(digits: string): boolean {
  const values = [...digits].map(digit => digit.charCodeAt(0) - 48);
  const check = values.pop() ?? -1;
  let sum = 0;
  for (let i = values.length - 1, weight = 3; i >= 0; i -= 1, weight = weight === 3 ? 1 : 3) sum += values[i] * weight;
  return (10 - (sum % 10)) % 10 === check;
}

function isbn10(value: string): boolean {
  let sum = 0;
  for (let i = 0; i < value.length; i += 1) {
    const byte = value.charCodeAt(i);
    const digit = byte >= 48 && byte <= 57 ? byte - 48 : byte === 88 && i === 9 ? 10 : -1;
    if (digit < 0) return false;
    sum += digit * (10 - i);
  }
  return sum % 11 === 0;
}

function validCode(key: string, value: string): boolean {
  if (key === "isbn" && value.length === 10) return isbn10(value);
  const lengths = CODE_LENGTHS[key] ?? [];
  return lengths.includes(value.length) && /^\d+$/.test(value) && gtinCheck(value);
}

/// Rows to show for specifics already on the listing. One blank row when there are none.
export function specificRows(specifics: Record<string, string> | undefined): SpecificRow[] {
  const rows = Object.entries(specifics ?? {}).map(([key, value]) => ({ key, value }));
  return rows.length ? rows : [{ key: "", value: "" }];
}

export function returnsDraft(returns: Returns | undefined): ReturnsDraft {
  if (returns?.kind === "none") return { kind: "none", days: "", sellerPays: false };
  if (returns?.kind === "days") return { kind: "days", days: String(returns.days), sellerPays: returns.sellerPays };
  return { kind: "store", days: "", sellerPays: false };
}

/// Item specifics and the returns override the listing signs.
export function listingFacts(rows: SpecificRow[], returns: ReturnsDraft): ListingFacts | string {
  const specifics = new Map<string, string>();
  for (const row of rows) {
    const key = specificKey(row.key);
    const value = row.value.trim();
    if (!key && !value) continue;
    if (!key) return "Name each item specific.";
    if (!/^[a-z0-9-]+$/.test(key) || bytes(key) > 32) return "An item specific key is lowercase letters, digits and dashes.";
    if (!value || bytes(value) > 120) return "An item specific value is empty or too long.";
    if (specifics.has(key)) return "Each item specific is named once.";
    if ((key === "gtin" || key === "upc" || key === "ean" || key === "isbn") && !validCode(key, value)) return "A product code's check digit is wrong.";
    specifics.set(key, value);
  }
  if (specifics.size > 24) return "A listing has at most 24 item specifics.";
  let signedReturns: Returns = { kind: "store" };
  if (returns.kind === "none") signedReturns = { kind: "none" };
  if (returns.kind === "days") {
    if (!/^\d+$/.test(returns.days.trim())) return "A returns window is 1 to 365 days.";
    const days = Number(returns.days.trim());
    if (days < 1 || days > 365) return "A returns window is 1 to 365 days.";
    signedReturns = { kind: "days", days, sellerPays: returns.sellerPays };
  }
  const ordered: Record<string, string> = {};
  for (const key of [...specifics.keys()].sort()) ordered[key] = specifics.get(key) ?? "";
  return { specifics: ordered, returns: signedReturns };
}

/// Put specifics and returns on the commerce block the existing sign path already accepts.
export function withFacts(commerce: Commerce | null, facts: ListingFacts): Commerce | null {
  const bare = Object.keys(facts.specifics).length === 0 && facts.returns.kind === "store";
  if (!commerce && bare) return null;
  return { ...(commerce ?? emptyCommerce()), specifics: facts.specifics, returns: facts.returns };
}

/// Shipping and pickup only make sense for physical goods.
export function forKind(commerce: Commerce, kind: "physical" | "digital"): Commerce {
  return kind === "digital" ? { ...commerce, shipping: { weightG: 0, dimsMm: [0, 0, 0], handlingDays: 0, freeOver: 0, services: [], pickup: null } } : commerce;
}
