// Choosing a variant: which option values can still be picked given the
// others, which combination is selected, and how to word its stock.
//
// Stock comes from the signed listing, so it is "as of" the day the seller
// last published; the seller's node has the final word when it reads the order.

import type { Availability, Commerce, Variant } from "./commerce.ts";

/// One entry per axis: the index of the chosen value, or null.
export type Selection = (number | null)[];

/// "unavailable": no variant has this value together with the other choices.
export type OptionState = "available" | "sold-out" | "unavailable";

export const LOW_STOCK = 3;

function choiceOf(commerce: Commerce, variant: Variant): number[] {
  return variant.options.map((value, axis) => commerce.axes[axis].values.indexOf(value));
}

/// Units this variant can sell, or Infinity when it is made to order with no stock cap.
export function sellable(commerce: Commerce, variant: Variant): number {
  return commerce.availability.kind === "made-to-order" && variant.stock === 0 ? Infinity : variant.stock;
}

function matches(choice: number[], selection: Selection, skip: number): boolean {
  return choice.every((index, axis) => axis === skip || selection[axis] === null || selection[axis] === index);
}

/// The state of every value on every axis, given what is chosen on the other axes.
export function optionStates(commerce: Commerce, selection: Selection): OptionState[][] {
  const choices = commerce.variants.map(v => ({ v, choice: choiceOf(commerce, v) }));
  return commerce.axes.map((axis, a) =>
    axis.values.map((_, value) => {
      const candidates = choices.filter(({ choice }) => choice[a] === value && matches(choice, selection, a));
      if (!candidates.length) return "unavailable";
      return candidates.some(({ v }) => sellable(commerce, v) > 0) ? "available" : "sold-out";
    }),
  );
}

export function selectedVariant(commerce: Commerce, selection: Selection): Variant | null {
  if (selection.some(index => index === null)) return null;
  return commerce.variants.find(v => choiceOf(commerce, v).every((index, axis) => index === selection[axis])) ?? null;
}

/// Pick `value` on `axis`. Choices on other axes that no longer form an
/// offered combination are cleared, so the buyer is never left on one.
export function choose(commerce: Commerce, selection: Selection, axis: number, value: number): Selection {
  const next = selection.map((index, a) => (a === axis ? value : index));
  const offered = commerce.variants.map(v => choiceOf(commerce, v));
  return next.map((index, a) => {
    if (a === axis || index === null) return index;
    const trial = next.map((x, b) => (b === a ? index : b === axis ? value : null));
    return offered.some(choice => choice.every((x, b) => trial[b] === null || trial[b] === x)) ? index : null;
  });
}

/// Start on the first variant that can be bought, or with nothing chosen.
export function initialSelection(commerce: Commerce): Selection {
  const first = commerce.variants.find(v => sellable(commerce, v) > 0);
  return first ? choiceOf(commerce, first) : commerce.axes.map(() => null);
}

/// Total sellable units across variants (for a listing card).
export function listingSellable(commerce: Commerce | null, stock: number): number {
  if (!commerce || commerce.variants.length === 0) {
    return commerce?.availability.kind === "made-to-order" && stock === 0 ? Infinity : stock;
  }
  return commerce.variants.reduce((n, v) => n + sellable(commerce, v), 0);
}

function dayLabel(day: number): string {
  const date = new Date(Date.UTC(Math.floor(day / 10_000), Math.floor(day / 100) % 100 - 1, day % 100));
  return date.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}

export type StockNote = { tone: "ok" | "low" | "out" | "info"; text: string };

/// How the listing page words stock. `asOf` is the day the listing was
/// signed, or 0 when that is not known.
export function stockNote(available: number, availability: Availability, asOf: number): StockNote {
  if (availability.kind === "away") return { tone: "out", text: "The seller is away and not taking orders" };
  if (availability.kind === "made-to-order" && available === Infinity) return { tone: "info", text: `Made to order · ships in ${availability.days} days` };
  if (available <= 0) return { tone: "out", text: "Sold out" };
  const when = asOf ? `as of ${dayLabel(asOf)}` : "as of the seller's last update";
  if (availability.kind === "pre-order") return { tone: "info", text: `Pre-order · ships ${dayLabel(availability.shipsOn)} · ${available} left ${when}` };
  if (available <= LOW_STOCK) return { tone: "low", text: `Only ${available} left ${when}` };
  return { tone: "ok", text: `${available} in stock ${when}` };
}

/// Most a buyer can put in one order of this variant.
export function maxQuantity(commerce: Commerce | null, available: number): number {
  const limit = commerce?.maxPerOrder || 99;
  return Math.max(0, Math.min(limit, available, 99));
}
