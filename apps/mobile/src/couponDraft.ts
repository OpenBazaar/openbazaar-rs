/** The coupon body `POST /coupons` signs, or the reason the record would refuse it. */

export type CouponForm = {
  code: string;
  kind: "percent" | "fixed";
  amount: string;
  currency: string;
  minSubtotal: string;
  startsOn: string;
  endsOn: string;
  maxUses: string;
  listings: string[];
};

export type CouponBody = {
  code: string;
  percentBps?: number;
  fixed?: number;
  currency: string;
  minSubtotal: number;
  startsOn: number;
  endsOn: number;
  maxUses: number;
  listings: string[];
};

const CODE = "A coupon code is 3 to 24 uppercase letters, digits or dashes.";

function dayNumber(text: string): number | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text.trim());
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (year < 2000 || month < 1 || month > 12 || day < 1 || day > 31) return null;
  return year * 10_000 + month * 100 + day;
}

function whole(text: string, empty: number, label: string): number | string {
  const value = text.trim();
  if (!value) return empty;
  if (!/^\d+$/.test(value)) return `The ${label} must be a whole number.`;
  return Number(value);
}

export function couponDraft(form: CouponForm): CouponBody | string {
  const code = form.code.trim().toUpperCase();
  if (!/^[A-Z0-9-]{3,24}$/.test(code)) return CODE;
  const currency = form.currency.trim().toUpperCase();
  if (!/^[A-Z]{3}$/.test(currency)) return "A currency is three uppercase letters.";
  const amount = whole(form.amount, NaN, form.kind === "percent" ? "discount" : "amount");
  if (typeof amount === "string") return amount;
  if (!Number.isInteger(amount)) return form.kind === "percent" ? "A coupon takes 0.01% to 100% off." : "A coupon takes more than zero off.";
  if (form.kind === "percent" && (amount < 1 || amount > 10_000)) return "A coupon takes 0.01% to 100% off.";
  if (form.kind === "fixed" && amount < 1) return "A coupon takes more than zero off.";
  const minSubtotal = whole(form.minSubtotal, 0, "minimum");
  if (typeof minSubtotal === "string") return minSubtotal;
  const maxUses = whole(form.maxUses, 0, "use limit");
  if (typeof maxUses === "string") return maxUses;
  const startsOn = dayNumber(form.startsOn);
  const endsOn = dayNumber(form.endsOn);
  if (startsOn == null || endsOn == null) return "A coupon date is written YYYY-MM-DD.";
  if (startsOn > endsOn) return "A coupon ends before it starts.";
  return {
    code,
    ...(form.kind === "percent" ? { percentBps: amount } : { fixed: amount }),
    currency,
    minSubtotal,
    startsOn,
    endsOn,
    maxUses,
    listings: form.listings,
  };
}
