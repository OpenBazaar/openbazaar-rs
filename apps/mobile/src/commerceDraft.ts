// The seller's editor model for a listing's options and terms.
//
// The editor works on the same Commerce shape the node returns and accepts.
// Variants are generated from the option axes: every combination gets a row,
// and a row the seller switches off is left out of the signed listing (it is
// "not offered"). Existing rows keep their id, so orders placed against an
// earlier version still find their variant after an edit.

import type { Axis, Commerce, Service, Variant } from "./commerce.ts";

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

/// Shipping and pickup only make sense for physical goods.
export function forKind(commerce: Commerce, kind: "physical" | "digital"): Commerce {
  return kind === "digital" ? { ...commerce, shipping: { weightG: 0, dimsMm: [0, 0, 0], handlingDays: 0, freeOver: 0, services: [], pickup: null } } : commerce;
}
