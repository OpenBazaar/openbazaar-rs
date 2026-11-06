// Client-side content filtering. It runs on this device only: nothing here is
// sent to the node or the network, and nothing is removed from either.
import type { ContentRating, ListingHeader } from "../catalog";
import { SENSITIVE_KEYWORDS } from "./sensitiveKeywords.ts";

export type SafetySettings = {
  /** Hide listings rated sensitive or adult, and unlabeled ones that match the built-in list. */
  safeMode: boolean;
  /** The buyer's own words. Matching listings are hidden whether or not safe mode is on. */
  blockedKeywords: string[];
};

export const DEFAULT_SAFETY: SafetySettings = { safeMode: true, blockedKeywords: [] };

export type Verdict = { hidden: false } | { hidden: true; reason: string };

const SHOWN: Verdict = { hidden: false };
const RANK: Record<ContentRating, number> = { unrated: 0, general: 0, sensitive: 1, adult: 2 };

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

/**
 * Decide whether this device shows a listing.
 *
 * A store rating raises every listing in it: an adult store hides all of its
 * listings in safe mode, whatever each listing says.
 */
export function safetyVerdict(
  listing: ListingHeader,
  storeRating: ContentRating,
  settings: SafetySettings,
  builtIn: readonly string[] = SENSITIVE_KEYWORDS,
): Verdict {
  const blocked = matchedKeyword(listing, settings.blockedKeywords);
  if (blocked) return { hidden: true, reason: `It matches your blocked word “${blocked}”.` };
  if (!settings.safeMode) return SHOWN;
  const rating = RANK[storeRating] > RANK[listing.contentRating] ? storeRating : listing.contentRating;
  if (rating === "adult" || rating === "sensitive") {
    const who = rating === storeRating && rating !== listing.contentRating ? "store" : "seller";
    return { hidden: true, reason: `The ${who} rated it ${rating}.` };
  }
  const flagged = matchedKeyword(listing, builtIn);
  if (flagged) return { hidden: true, reason: `It mentions “${flagged}”, which safe mode treats as sensitive.` };
  return SHOWN;
}

export function visibleListings(
  listings: readonly ListingHeader[],
  storeRating: ContentRating,
  settings: SafetySettings,
): ListingHeader[] {
  return listings.filter((listing) => !safetyVerdict(listing, storeRating, settings).hidden);
}

/** Read stored settings. Anything missing or unreadable falls back to safe mode on. */
export function parseSafety(raw: string | null): SafetySettings {
  if (!raw) return DEFAULT_SAFETY;
  try {
    const value = JSON.parse(raw) as { safeMode?: unknown; blockedKeywords?: unknown };
    return {
      safeMode: value.safeMode !== false,
      blockedKeywords: Array.isArray(value.blockedKeywords)
        ? parseKeywords(value.blockedKeywords.filter((term): term is string => typeof term === "string").join("\n"))
        : [],
    };
  } catch {
    return DEFAULT_SAFETY;
  }
}
