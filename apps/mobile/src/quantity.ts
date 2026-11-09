/** The count a buyer sends on a checkout line the node already accepts. */
export function checkoutLine(item: { listingHash: string; quantity: number; variant?: string }): { listingHash: string; quantity: number; variant?: string } {
  const line: { listingHash: string; quantity: number; variant?: string } = { listingHash: item.listingHash, quantity: item.quantity };
  if (item.variant) line.variant = item.variant;
  return line;
}

/// One more or one fewer, staying inside 1..min(stock, 99). A step past either end stays put.
export function nextQuantity(current: number, delta: number, max: number): number {
  const cap = Math.min(99, Number.isFinite(max) ? max : 99);
  const next = current + delta;
  if (!Number.isInteger(next) || next < 1 || next > cap) return current;
  return next;
}

/// Zero, a negative count, a fraction, more than 99, or more than stock (when stock is a real cap) cannot be ordered.
export function canOrder(quantity: number, available: number): boolean {
  if (!Number.isInteger(quantity) || quantity < 1 || quantity > 99) return false;
  if (Number.isFinite(available) && quantity > available) return false;
  return true;
}
