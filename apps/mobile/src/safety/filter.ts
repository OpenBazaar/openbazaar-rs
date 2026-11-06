// Client-side content filtering. It runs on this device only: nothing here is
// sent to the node or the network, and nothing is removed from either.
import type { ContentRating, ListingHeader } from "../catalog";
import { SENSITIVE_KEYWORDS } from "./sensitiveKeywords.ts";

export const CONTENT_TAGS = ["nudity", "violence", "drugs", "weapons", "gambling"] as const;
export type ContentTag = (typeof CONTENT_TAGS)[number];
export type SensitiveMode = "show" | "cover" | "hide";
export type AdultMode = "show" | "hide";
export type TagMode = "show" | "hide";

export type SafetySettings = {
  /** Master switch for ratings, tags, and the built-in word list. */
  safeMode: boolean;
  /** What safe mode does with a sensitive listing. "cover" keeps a placeholder the buyer can open. */
  sensitive: SensitiveMode;
  adult: AdultMode;
  /** Each tag on its own, on top of the rating. */
  tags: Record<ContentTag, TagMode>;
  /** The buyer's own words. Matching listings are hidden whether or not safe mode is on. */
  blockedKeywords: string[];
  /** Stores the buyer hid, by signing key. The label is only for the undo list. */
  hiddenStores: Hidden[];
  /** Listings the buyer hid, by content hash. */
  hiddenListings: Hidden[];
};

export type Hidden = { id: string; label: string };

export const DEFAULT_SAFETY: SafetySettings = {
  safeMode: true,
  sensitive: "hide",
  adult: "hide",
  tags: { nudity: "show", violence: "show", drugs: "show", weapons: "show", gambling: "show" },
  blockedKeywords: [],
  hiddenStores: [],
  hiddenListings: [],
};

export type Verdict = { action: "show" } | { action: "cover" | "hide"; reason: string };

const SHOWN: Verdict = { action: "show" };
const RANK: Record<ContentRating, number> = { unrated: 0, general: 0, sensitive: 1, adult: 2 };
const STRENGTH: Record<Verdict["action"], number> = { show: 0, cover: 1, hide: 2 };

/** Split text into lowercase words. Anything that is not a letter or digit separates words. */
export function words(text: string): string[] {
  return text.normalize("NFKC").toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/** Parse a typed blocklist: one term per line or comma. Terms may be phrases. */
export function parseKeywords(text: string): string[] {
  const terms = text
    .split(/[\n,]/)
    .map((term) => words(term).join(" "))
    .filter(Boolean);
  return [...new Set(terms)];
}

/** The first term that appears as whole words in the listing's title, category, description, or tags. */
export function matchedKeyword(listing: ListingHeader, terms: readonly string[]): string | null {
  if (terms.length === 0) return null;
  const haystack = [listing.title, listing.category, listing.description ?? "", ...listing.contentTags].map(words);
  for (const term of terms) {
    const needle = words(term);
    if (needle.length > 0 && haystack.some((field) => containsRun(field, needle))) return term;
  }
  return null;
}

function containsRun(field: readonly string[], needle: readonly string[]): boolean {
  for (let start = 0; start + needle.length <= field.length; start += 1) {
    if (needle.every((word, offset) => field[start + offset] === word)) return true;
  }
  return false;
}

function strictest(verdicts: readonly Verdict[]): Verdict {
  return verdicts.reduce((worst, next) => (STRENGTH[next.action] > STRENGTH[worst.action] ? next : worst), SHOWN);
}

function forSensitive(mode: SensitiveMode, reason: string): Verdict {
  return mode === "show" ? SHOWN : { action: mode, reason };
}

/**
 * Decide whether this device shows, covers, or hides a listing.
 *
 * A store rating raises every listing in it. When several rules apply, the
 * strictest one wins and its reason is the one shown.
 */
export function safetyVerdict(
  listing: ListingHeader,
  storeRating: ContentRating,
  settings: SafetySettings,
  builtIn: readonly string[] = SENSITIVE_KEYWORDS,
): Verdict {
  if (settings.hiddenListings.some((item) => item.id === listing.hash)) return { action: "hide", reason: "You hid this listing." };
  if (settings.hiddenStores.some((item) => item.id === listing.storeId)) return { action: "hide", reason: "You hid this seller." };
  const blocked = matchedKeyword(listing, settings.blockedKeywords);
  if (blocked) return { action: "hide", reason: `It matches your blocked word “${blocked}”.` };
  if (!settings.safeMode) return SHOWN;
  const verdicts: Verdict[] = [];
  const rating = RANK[storeRating] > RANK[listing.contentRating] ? storeRating : listing.contentRating;
  const who = rating === storeRating && rating !== listing.contentRating ? "store" : "seller";
  if (rating === "adult" && settings.adult === "hide") verdicts.push({ action: "hide", reason: `The ${who} rated it adult.` });
  if (rating === "sensitive") verdicts.push(forSensitive(settings.sensitive, `The ${who} rated it sensitive.`));
  for (const tag of listing.contentTags) {
    if (settings.tags[tag as ContentTag] === "hide") verdicts.push({ action: "hide", reason: `It is tagged ${tag}, which you hide.` });
  }
  const flagged = matchedKeyword(listing, builtIn);
  if (flagged) verdicts.push(forSensitive(settings.sensitive, `It mentions “${flagged}”, which safe mode treats as sensitive.`));
  return strictest(verdicts);
}

export function visibleListings(
  listings: readonly ListingHeader[],
  storeRating: ContentRating,
  settings: SafetySettings,
): ListingHeader[] {
  return listings.filter((listing) => safetyVerdict(listing, storeRating, settings).action !== "hide");
}

function pick<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return allowed.includes(value as T) ? (value as T) : fallback;
}

/**
 * Read stored settings. Anything missing or unreadable falls back to the
 * default, so the first version's `{ safeMode, blockedKeywords }` reads as-is
 * with the new switches at their defaults.
 */
export function parseSafety(raw: string | null): SafetySettings {
  if (!raw) return DEFAULT_SAFETY;
  let value: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return DEFAULT_SAFETY;
    value = parsed as Record<string, unknown>;
  } catch {
    return DEFAULT_SAFETY;
  }
  const tags = (value.tags && typeof value.tags === "object" ? value.tags : {}) as Record<string, unknown>;
  return {
    safeMode: value.safeMode !== false,
    sensitive: pick(value.sensitive, ["show", "cover", "hide"], DEFAULT_SAFETY.sensitive),
    adult: pick(value.adult, ["show", "hide"], DEFAULT_SAFETY.adult),
    tags: Object.fromEntries(CONTENT_TAGS.map((tag) => [tag, pick(tags[tag], ["show", "hide"], DEFAULT_SAFETY.tags[tag])])) as Record<ContentTag, TagMode>,
    blockedKeywords: strings(value.blockedKeywords, (list) => parseKeywords(list.join("\n"))),
    hiddenStores: hiddenList(value.hiddenStores),
    hiddenListings: hiddenList(value.hiddenListings),
  };
}

/** Add or remove one entry, keyed by id. */
export function toggleHidden(list: readonly Hidden[], item: Hidden): Hidden[] {
  return list.some((entry) => entry.id === item.id) ? list.filter((entry) => entry.id !== item.id) : [...list, item];
}

function hiddenList(value: unknown): Hidden[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const out: Hidden[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") continue;
    const { id, label } = item as Record<string, unknown>;
    if (typeof id !== "string" || !/^[0-9a-f]{64}$/.test(id) || seen.has(id)) continue;
    seen.add(id);
    out.push({ id, label: typeof label === "string" ? label : id.slice(0, 12) });
  }
  return out;
}

function strings(value: unknown, clean: (list: string[]) => string[] = (list) => [...new Set(list)]): string[] {
  return Array.isArray(value) ? clean(value.filter((item): item is string => typeof item === "string")) : [];
}
