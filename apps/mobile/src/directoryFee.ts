// This store's directory fee, from `GET /directory/fee`: the address any
// wallet, exchange, or swap can pay, the amount, and whether the store is listed.
import { paired } from "./catalog.ts";

export type DirectoryFeeState = "listed" | "confirming" | "unpaid" | "checking";

export type DirectoryFee =
  | { available: false; reason: string }
  | { available: true; address: string; sats: number; confirmations: number; state: DirectoryFeeState; listedUntilHeight: number | null };

const STATES: readonly DirectoryFeeState[] = ["listed", "confirming", "unpaid", "checking"];

export function directoryFeeOf(value: unknown): DirectoryFee {
  const item = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  if (item.available !== true || typeof item.address !== "string" || typeof item.sats !== "number") {
    return { available: false, reason: typeof item.reason === "string" ? item.reason : "The node did not say where to pay the directory fee." };
  }
  const state = STATES.find(known => known === item.state) ?? "checking";
  return {
    available: true,
    address: item.address,
    sats: item.sats,
    confirmations: typeof item.confirmations === "number" ? item.confirmations : 1,
    state,
    listedUntilHeight: typeof item.listedUntilHeight === "number" ? item.listedUntilHeight : null,
  };
}

export function fetchDirectoryFee(): Promise<DirectoryFee> {
  return paired("/directory/fee").then(directoryFeeOf);
}

export function directoryFeeStatus(fee: Extract<DirectoryFee, { available: true }>): string {
  const amount = `${fee.sats.toLocaleString("en-US")} sats`;
  switch (fee.state) {
    case "listed":
      return fee.listedUntilHeight === null
        ? "Your store is listed in the directory."
        : `Your store is listed in the directory until block ${fee.listedUntilHeight.toLocaleString("en-US")}. Pay again before then to stay listed.`;
    case "confirming":
      return `Payment seen. Your store is listed after ${fee.confirmations} confirmation${fee.confirmations === 1 ? "" : "s"}.`;
    case "unpaid":
      return `Not listed yet. Your node could not pay from its bitcoin wallet. Send ${amount} to this address from any wallet, exchange, or swap.`;
    case "checking":
      return "Checking for a payment. This takes up to a minute.";
  }
}
