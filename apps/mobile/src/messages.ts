/** Buyer–seller messages inside one order, as the node reports them. */

export type OrderMessage = {
  id: string;
  /** Who wrote it, from the order's point of view. */
  from: "buyer" | "seller";
  text: string;
  /** When the viewing side's node sent or received it, in seconds. */
  at: number;
};

const text = (value: unknown, max = 2000) => (typeof value === "string" ? value.slice(0, max) : "");
const count = (value: unknown) => (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0);

/** Parse a message list without letting a node invent senders or oversize text. */
export function parseMessages(value: unknown): OrderMessage[] {
  if (!Array.isArray(value)) return [];
  return value
    .flatMap((item) => {
      if (!item || typeof item !== "object") return [];
      const message = item as Record<string, unknown>;
      const from: "buyer" | "seller" | null = message.from === "buyer" ? "buyer" : message.from === "seller" ? "seller" : null;
      const body = text(message.text);
      if (!from || !body) return [];
      return [{ id: text(message.id, 64), from, text: body, at: count(message.at) }];
    })
    .slice(-500);
}
