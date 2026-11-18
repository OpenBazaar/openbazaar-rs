import { paired } from "./catalog.ts";
import { parseFeeKind, type FeeKind } from "./disputeFee.ts";

// Moderators as the node knows them. A moderator is its key: the node checks
// every profile's signature before it lists one, and the name is only a
// label. The track record is what the node counted from rulings that key
// signed, never a number the moderator claims.

/** Filing-fee cases, counted from the case-accepted and decision records the key signed. */
export type CaseCounts = { accepted: number; decided: number; denied: number; overdue: number };

export type TrackRecord = { rulings: number; stores: number; checkedAt: number; cases: CaseCounts };

export type KnownModerator = {
  /** The moderator's key, in hex. This, not the name, is who the moderator is. */
  id: string;
  name: string;
  description: string;
  terms: string;
  languages: string[];
  feeSats: number;
  /** `filing`: paid by whoever opens a dispute. `ruling`: an output of each ruling, from before filing fees. */
  feeKind: FeeKind;
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
  /** Why filers would pay this fee on-chain rather than over Lightning, if they would. */
  feeWarning: string | null;
  /** A signed profile is being served. */
  published: boolean;
  /** The address other nodes reach this one at, once it has one. */
  address: string | null;
  rulings: number;
  stores: number;
  cases: CaseCounts;
  builtIn: { id: string; name: string } | null;
};

export type ProfileEdit = { name: string; description: string; terms: string; languages: string[]; feeSats: number };

const record = (value: unknown) => (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
const text = (value: unknown) => (typeof value === "string" ? value : "");
const count = (value: unknown) => (typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0);
const casesOf = (value: unknown): CaseCounts => {
  const r = record(value);
  return { accepted: count(r.accepted), decided: count(r.decided), denied: count(r.denied), overdue: count(r.overdue) };
};
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
    feeKind: parseFeeKind(r.feeKind) ?? "ruling",
    address: typeof r.address === "string" ? r.address : null,
    builtIn: r.builtIn === true,
    accepted: r.accepted === true,
    trackRecord: track ? { rulings: count(track.rulings), stores: count(track.stores), checkedAt: count(track.checkedAt), cases: casesOf(track.cases) } : null,
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
    feeWarning: typeof r.feeWarning === "string" && r.feeWarning ? r.feeWarning : null,
    published: r.published,
    address: typeof r.address === "string" ? r.address : null,
    rulings: count(track.rulings),
    stores: count(track.stores),
    cases: casesOf(track.cases),
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
  const rulings = track.rulings ? `${track.rulings} signed public ${track.rulings === 1 ? "ruling" : "rulings"} across ${track.stores} ${track.stores === 1 ? "store" : "stores"}.` : "No public rulings yet.";
  const cases = caseCountsText(track.cases);
  return cases ? `${rulings} ${cases}` : rulings;
}

/** The filing-fee cases in words, or "" when there are none. */
export function caseCountsText(cases: CaseCounts): string {
  if (!cases.accepted && !cases.decided) return "";
  const parts = [`${cases.accepted} paid ${cases.accepted === 1 ? "case" : "cases"}`, `${cases.decided} decided`];
  if (cases.denied) parts.push(`${cases.denied} denied`);
  if (cases.overdue) parts.push(`${cases.overdue} overdue`);
  return `${parts.join(", ")}.`;
}

/** What one dispute with `moderator` costs, as a buyer or seller compares them. */
export function feeText(moderator: Pick<KnownModerator, "feeKind" | "feeSats">): string {
  if (moderator.feeKind === "ruling") return `Fee ${moderator.feeSats.toLocaleString()} sats if a dispute is ruled`;
  return moderator.feeSats ? `Opening a dispute costs ${moderator.feeSats.toLocaleString()} sats` : "No filing fee";
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
