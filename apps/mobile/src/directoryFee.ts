// This store's directory fee, from `GET /directory/fee`: the address any
// wallet, exchange, or swap can pay, the amount, and whether the store is listed.
import { paired } from "./catalog.ts";

export type DirectoryFeeState = "listed" | "confirming" | "unpaid" | "checking";

export type DirectoryTerms = { sats: number; confirmations: number; epochBlocks: number };

/** Terms the directory has scheduled from `startHeight`, announced at `announcedHeight`. */
export type ScheduledTerms = DirectoryTerms & { startHeight: number; announcedHeight: number; address: string; addressChanges: boolean };

export type DirectoryFee =
  | { available: false; reason: string }
  | (DirectoryTerms & {
    available: true;
    address: string;
    state: DirectoryFeeState;
    listedUntilHeight: number | null;
    upcoming: ScheduledTerms[];
    headRefused: { sequence: number; reason: string } | null;
  });

const STATES: readonly DirectoryFeeState[] = ["listed", "confirming", "unpaid", "checking"];
const BLOCKS_PER_DAY = 144;

function record(value: unknown): Record<string, unknown> {
  return (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
}

function number(value: unknown, fallback: number): number {
  return typeof value === "number" ? value : fallback;
}

function scheduledOf(value: unknown): ScheduledTerms | null {
  const item = record(value);
  if (typeof item.startHeight !== "number" || typeof item.sats !== "number" || typeof item.address !== "string") return null;
  return {
    startHeight: item.startHeight,
    announcedHeight: number(item.announcedHeight, item.startHeight),
    address: item.address,
    addressChanges: item.addressChanges === true,
    sats: item.sats,
    confirmations: number(item.confirmations, 1),
    epochBlocks: number(item.epochBlocks, 2016),
  };
}

function refusedOf(value: unknown): { sequence: number; reason: string } | null {
  const item = record(value);
  return typeof item.sequence === "number" && typeof item.reason === "string" ? { sequence: item.sequence, reason: item.reason } : null;
}

export function directoryFeeOf(value: unknown): DirectoryFee {
  const item = record(value);
  if (item.available !== true || typeof item.address !== "string" || typeof item.sats !== "number") {
    return { available: false, reason: typeof item.reason === "string" ? item.reason : "The node did not say where to pay the directory fee." };
  }
  const state = STATES.find(known => known === item.state) ?? "checking";
  return {
    available: true,
    address: item.address,
    sats: item.sats,
    confirmations: number(item.confirmations, 1),
    epochBlocks: number(item.epochBlocks, 2016),
    state,
    listedUntilHeight: typeof item.listedUntilHeight === "number" ? item.listedUntilHeight : null,
    upcoming: Array.isArray(item.upcoming) ? item.upcoming.map(scheduledOf).filter(entry => entry !== null) : [],
    headRefused: refusedOf(item.headRefused),
  };
}

export function fetchDirectoryFee(): Promise<DirectoryFee> {
  return paired("/directory/fee").then(directoryFeeOf);
}

type AvailableFee = Extract<DirectoryFee, { available: true }>;

function block(height: number): string {
  return `block ${height.toLocaleString("en-US")}`;
}

function sats(amount: number): string {
  return `${amount.toLocaleString("en-US")} sats`;
}

function plural(count: number, unit: string): string {
  return `${count} ${unit}${count === 1 ? "" : "s"}`;
}

/** How long one payment lists the store, in words: "two weeks" for 2,016 blocks. */
export function epochWords(blocks: number): string {
  const days = blocks / BLOCKS_PER_DAY;
  if (!Number.isInteger(days)) return plural(blocks, "block");
  if (days === 14) return "two weeks";
  return days % 7 === 0 ? plural(days / 7, "week") : plural(days, "day");
}

export function directoryFeeStatus(fee: AvailableFee): string {
  switch (fee.state) {
    case "listed":
      return fee.listedUntilHeight === null
        ? "Your store is listed in the directory."
        : `Your store is listed in the directory until ${block(fee.listedUntilHeight)}. Pay again before then to stay listed.`;
    case "confirming":
      return `Payment seen. Your store is listed after ${plural(fee.confirmations, "confirmation")}.`;
    case "unpaid":
      return `Not listed yet. Your node could not pay from its bitcoin wallet. Send ${sats(fee.sats)} to this address from any wallet, exchange, or swap.`;
    case "checking":
      return "Checking for a payment. This takes up to a minute.";
  }
}

function changeWords(before: DirectoryTerms, after: ScheduledTerms): string[] {
  const words: string[] = [];
  if (after.sats !== before.sats) words.push(`the fee ${after.sats > before.sats ? "rises" : "drops"} to ${sats(after.sats)}`);
  if (after.confirmations !== before.confirmations) words.push(`a payment needs ${plural(after.confirmations, "confirmation")}`);
  if (after.epochBlocks !== before.epochBlocks) words.push(`each payment lists the store for ${epochWords(after.epochBlocks)}`);
  if (after.addressChanges) words.push("the fee address changes");
  return words;
}

/** One sentence per scheduled change, each against the terms before it. */
export function directoryFeeChanges(fee: AvailableFee): string[] {
  let before: DirectoryTerms = fee;
  return fee.upcoming.map(entry => {
    const words = changeWords(before, entry);
    before = entry;
    const what = words.length === 0 ? "the terms stay the same" : words.length < 3 ? words.join(" and ") : `${words.slice(0, -1).join(", ")}, and ${words[words.length - 1]}`;
    return `From ${block(entry.startHeight)}, ${what}. The directory announced this at ${block(entry.announcedHeight)}.`;
  });
}

export function directoryHeadRefusal(fee: AvailableFee): string | null {
  if (!fee.headRefused) return null;
  return `Your node refused directory update ${fee.headRefused.sequence}: ${fee.headRefused.reason}. It keeps the terms shown here.`;
}
