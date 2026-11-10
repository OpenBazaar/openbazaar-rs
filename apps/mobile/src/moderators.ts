import { paired } from "./catalog.ts";

// Moderators as the node knows them. A moderator is its key: the node checks
// every profile's signature before it lists one, and the name is only a
// label. The track record is what the node counted from rulings that key
// signed, never a number the moderator claims.

export type TrackRecord = { rulings: number; stores: number; checkedAt: number };

export type KnownModerator = {
  /** The moderator's key, in hex. This, not the name, is who the moderator is. */
  id: string;
  name: string;
  description: string;
  terms: string;
  languages: string[];
  feeSats: number;
  /** Where dispute letters go. `null`: this node has no address for it. */
  address: string | null;
  /** The built-in test moderator, whose keys are public. */
  builtIn: boolean;
  /** Whether this node's stores accept it. */
  accepted: boolean;
  /** `null`: nothing verified has been counted. */
  trackRecord: TrackRecord | null;
};

/** This node's own moderator identity, as its desk edits it. */
export type OwnModerator = {
  /** `null` until the wallet is first unlocked on a moderating node. */
  id: string | null;
  name: string;
  description: string;
  terms: string;
  languages: string[];
  feeSats: number;
  minFeeSats: number;
  /** A signed profile is being served. */
  published: boolean;
  /** The address other nodes reach this one at, once it has one. */
  address: string | null;
  rulings: number;
  stores: number;
  builtIn: { id: string; name: string } | null;
};

export type ProfileEdit = { name: string; description: string; terms: string; languages: string[]; feeSats: number };

const record = (value: unknown) => (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
const text = (value: unknown) => (typeof value === "string" ? value : "");
const count = (value: unknown) => (typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0);
const tags = (value: unknown) => (Array.isArray(value) ? value.filter((tag): tag is string => typeof tag === "string") : []);

function parseModerator(value: unknown): KnownModerator {
  const r = record(value);
  if (typeof r.id !== "string" || !/^[0-9a-f]{64}$/.test(r.id)) throw new Error("The node returned an unreadable moderator.");
  const track = r.trackRecord && typeof r.trackRecord === "object" ? record(r.trackRecord) : null;
  return {
    id: r.id,
    name: text(r.name),
    description: text(r.description),
    terms: text(r.terms),
    languages: tags(r.languages),
    feeSats: count(r.feeSats),
    address: typeof r.address === "string" ? r.address : null,
    builtIn: r.builtIn === true,
    accepted: r.accepted === true,
    trackRecord: track ? { rulings: count(track.rulings), stores: count(track.stores), checkedAt: count(track.checkedAt) } : null,
  };
}

export function parseModerators(value: unknown): KnownModerator[] {
  const list = record(value).moderators;
  if (!Array.isArray(list)) throw new Error("The node returned an unreadable list of moderators.");
  return list.map(parseModerator);
}

export function parseOwnModerator(value: unknown): OwnModerator {
  const r = record(value);
  if (typeof r.published !== "boolean") throw new Error("The node returned an unreadable moderator profile.");
  const track = record(r.trackRecord);
  const builtIn = record(r.builtIn);
  return {
    id: typeof r.id === "string" ? r.id : null,
    name: text(r.name),
    description: text(r.description),
    terms: text(r.terms),
    languages: tags(r.languages),
    feeSats: count(r.feeSats),
    minFeeSats: count(r.minFeeSats),
    published: r.published,
    address: typeof r.address === "string" ? r.address : null,
    rulings: count(track.rulings),
    stores: count(track.stores),
    builtIn: typeof builtIn.id === "string" ? { id: builtIn.id, name: text(builtIn.name) } : null,
  };
}

/** A key short enough to read aloud and compare. */
export const shortKey = (id: string) => `${id.slice(0, 8)}…${id.slice(-6)}`;

/** What the node could verify about a moderator's past rulings, in words. */
export function trackRecordText(moderator: Pick<KnownModerator, "builtIn" | "trackRecord">): string {
  if (moderator.builtIn) return "Test moderator with public keys. No track record.";
  const track = moderator.trackRecord;
  if (!track) return "Track record not checked yet.";
  if (!track.rulings) return "No public rulings yet.";
  return `${track.rulings} signed public ${track.rulings === 1 ? "ruling" : "rulings"} across ${track.stores} ${track.stores === 1 ? "store" : "stores"}.`;
}

/** Language tags as the owner types them: "en, pt-br". */
export const languagesOf = (typed: string) => [...new Set(typed.split(/[\s,]+/).map(tag => tag.trim().toLowerCase()).filter(Boolean))];

const json = (method: string, body: unknown): RequestInit => ({ method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });

export async function fetchOwnModerator(): Promise<OwnModerator> {
  return parseOwnModerator(await paired("/moderator/profile"));
}

/** Sign the profile again with these fields. The node's wallet must be unlocked. */
export async function saveOwnModerator(edit: ProfileEdit): Promise<OwnModerator> {
  return parseOwnModerator(await paired("/moderator/profile", json("PUT", edit)));
}

/** The moderators this node's stores accept, then the others it knows. */
export async function fetchModerators(): Promise<KnownModerator[]> {
  return parseModerators(await paired("/moderators"));
}

/** Accept a moderator by the address it gives out, or one the node already knows by its key. */
export async function acceptModerator(by: { address: string } | { id: string }): Promise<KnownModerator[]> {
  return parseModerators(await paired("/moderators", json("POST", by)));
}

export async function removeModerator(id: string): Promise<KnownModerator[]> {
  return parseModerators(await paired(`/moderators/${id}`, { method: "DELETE" }));
}

/** The moderators a store's signed record accepts, for the buyer to choose from. */
export async function fetchStoreModerators(storeId: string): Promise<KnownModerator[]> {
  return parseModerators(await paired(`/stores/${storeId}/moderators`));
}
