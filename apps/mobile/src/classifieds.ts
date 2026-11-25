// Classifieds board and contact threads against the paired node.
import { paired } from "./catalog.ts";

export const CLASSIFIEDS_ROUTE = "/classifieds";
export const CONTACT_ROUTE = "/contact";

export type ClassifiedKind = "wtb" | "wts";
export type ClassifiedCategory = "goods" | "services" | "housing" | "jobs" | "other";

export type Classified = {
  id: string;
  kind: ClassifiedKind;
  category: ClassifiedCategory;
  title: string;
  body: string;
  region: string;
  priceHint: number;
  currency: string;
  createdOn: number;
  expiresOn: number;
  lifeDays: number;
  expired: boolean;
  poster: string;
  mailboxKey: string;
  onion: string;
};

export type ClassifiedDraft = {
  kind: ClassifiedKind;
  category: ClassifiedCategory;
  title: string;
  body: string;
  region?: string;
  priceHint?: string;
  currency?: string;
  lifeDays?: number;
};

export type ContactMessage = { id: string; from: "me" | "them"; text: string; at: number };
export type ContactThread = {
  id: string;
  classifiedId: string;
  poster: string;
  peer: string;
  asPoster: boolean;
  title: string;
  messages: ContactMessage[];
};

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("The node sent classifieds the page could not read.");
  return value as Record<string, unknown>;
}

function parseClassified(value: unknown): Classified {
  const row = record(value);
  const kind = row.kind;
  const category = row.category;
  if (kind !== "wtb" && kind !== "wts") throw new Error("The node sent classifieds the page could not read.");
  if (category !== "goods" && category !== "services" && category !== "housing" && category !== "jobs" && category !== "other") {
    throw new Error("The node sent classifieds the page could not read.");
  }
  const id = row.id;
  const title = row.title;
  const body = row.body;
  const region = row.region;
  const currency = row.currency;
  const poster = row.poster;
  const mailboxKey = row.mailboxKey;
  const onion = row.onion;
  if (
    typeof id !== "string" ||
    typeof title !== "string" ||
    typeof body !== "string" ||
    typeof region !== "string" ||
    typeof currency !== "string" ||
    typeof poster !== "string" ||
    typeof mailboxKey !== "string" ||
    typeof onion !== "string"
  ) {
    throw new Error("The node sent classifieds the page could not read.");
  }
  const priceHint = row.priceHint;
  const createdOn = row.createdOn;
  const expiresOn = row.expiresOn;
  const lifeDays = row.lifeDays;
  const expired = row.expired;
  if (
    !Number.isInteger(priceHint) ||
    !Number.isInteger(createdOn) ||
    !Number.isInteger(expiresOn) ||
    !Number.isInteger(lifeDays) ||
    typeof expired !== "boolean"
  ) {
    throw new Error("The node sent classifieds the page could not read.");
  }
  return {
    id,
    kind,
    category,
    title,
    body,
    region,
    priceHint: priceHint as number,
    currency,
    createdOn: createdOn as number,
    expiresOn: expiresOn as number,
    lifeDays: lifeDays as number,
    expired,
    poster,
    mailboxKey,
    onion,
  };
}

export function parseClassifieds(value: unknown): Classified[] {
  const items = record(value).classifieds;
  if (!Array.isArray(items)) return [];
  return items.map(parseClassified);
}

export async function fetchClassifieds(filters?: { kind?: ClassifiedKind; category?: ClassifiedCategory }): Promise<Classified[]> {
  const query = new URLSearchParams();
  if (filters?.kind) query.set("kind", filters.kind);
  if (filters?.category) query.set("category", filters.category);
  const suffix = query.toString() ? `?${query}` : "";
  return parseClassifieds(await paired(`${CLASSIFIEDS_ROUTE}${suffix}`));
}

export function classifiedSubmission(draft: ClassifiedDraft): { path: string; body: Record<string, unknown> } {
  const title = draft.title.trim();
  const body = draft.body.trim();
  if (!title) throw new Error("Give the post a title.");
  if (!body) throw new Error("Write a short description.");
  const lifeDays = draft.lifeDays ?? 14;
  if (!Number.isInteger(lifeDays) || lifeDays < 1 || lifeDays > 30) throw new Error("A classified lives between 1 and 30 days.");
  const payload: Record<string, unknown> = {
    kind: draft.kind,
    category: draft.category,
    title,
    body,
    lifeDays,
  };
  const region = draft.region?.trim();
  if (region) payload.region = region;
  const amount = draft.priceHint?.trim();
  if (amount) {
    const priceHint = Number(amount);
    if (!Number.isInteger(priceHint) || priceHint <= 0) throw new Error("Enter a whole-number price hint in minor units, or leave it blank.");
    const currency = (draft.currency ?? "USD").trim().toUpperCase();
    if (currency.length !== 3) throw new Error("A price hint needs a three-letter currency.");
    payload.priceHint = priceHint;
    payload.currency = currency;
  }
  return { path: CLASSIFIEDS_ROUTE, body: payload };
}

export async function createClassified(draft: ClassifiedDraft): Promise<Classified> {
  const { path, body } = classifiedSubmission(draft);
  return parseClassified(await paired(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }));
}

export async function deleteClassified(id: string): Promise<void> {
  if (!/^[0-9a-f]{32}$/i.test(id)) throw new Error("That classified is not on this node.");
  await paired(`${CLASSIFIEDS_ROUTE}/${id}`, { method: "DELETE" });
}

function parseMessage(value: unknown): ContactMessage {
  const row = record(value);
  const id = row.id;
  const from = row.from;
  const text = row.text;
  const at = row.at;
  if (typeof id !== "string" || (from !== "me" && from !== "them") || typeof text !== "string" || !Number.isInteger(at)) {
    throw new Error("The node sent a contact thread the page could not read.");
  }
  return { id, from, text, at: at as number };
}

export function parseContactThread(value: unknown): ContactThread {
  const row = record(value);
  const id = row.id;
  const classifiedId = row.classifiedId;
  const poster = row.poster;
  const peer = row.peer;
  const title = row.title;
  const asPoster = row.asPoster;
  const messages = row.messages;
  if (
    typeof id !== "string" ||
    typeof classifiedId !== "string" ||
    typeof poster !== "string" ||
    typeof peer !== "string" ||
    typeof title !== "string" ||
    typeof asPoster !== "boolean" ||
    !Array.isArray(messages)
  ) {
    throw new Error("The node sent a contact thread the page could not read.");
  }
  return { id, classifiedId, poster, peer, asPoster, title, messages: messages.map(parseMessage) };
}

export function parseContactThreads(value: unknown): ContactThread[] {
  const items = record(value).threads;
  if (!Array.isArray(items)) return [];
  return items.map(parseContactThread);
}

export async function fetchContactThreads(): Promise<ContactThread[]> {
  return parseContactThreads(await paired(CONTACT_ROUTE));
}

export async function contactClassified(id: string, text: string): Promise<ContactThread> {
  if (!/^[0-9a-f]{32}$/i.test(id)) throw new Error("That classified is not on this node.");
  const trimmed = text.trim();
  if (!trimmed) throw new Error("Write a message first.");
  return parseContactThread(
    await paired(`${CLASSIFIEDS_ROUTE}/${id}/contact`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ text: trimmed }),
    }),
  );
}

export async function replyContact(id: string, text: string): Promise<ContactThread> {
  if (!/^[0-9a-f]{32}$/i.test(id)) throw new Error("That contact thread is not on this node.");
  const trimmed = text.trim();
  if (!trimmed) throw new Error("Write a message first.");
  return parseContactThread(
    await paired(`${CONTACT_ROUTE}/${id}/message`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ text: trimmed }),
    }),
  );
}
