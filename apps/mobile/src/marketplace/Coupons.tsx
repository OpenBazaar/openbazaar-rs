import { useState, type FormEvent } from "react";
import { couponEditorDraft, type CouponForm } from "../couponDraft";
import { fetchCoupons, saveCoupon, formatPrice, type PriceCurrency } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { PageHeading } from "./PageHeading";
import { useLockedAction } from "./SellerKit";

const EMPTY: CouponForm = { code: "", kind: "percent", amount: "", currency: "USD", minSubtotal: "", startsOn: "", endsOn: "", maxUses: "", listings: [] };
function dateLabel(day: number) {
  const text = String(day);
  if (!/^\d{8}$/.test(text)) return '—';
  return new Date(Number(text.slice(0,4)), Number(text.slice(4,6)) - 1, Number(text.slice(6,8))).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

/** Codes the store already signs. A coupon is not a field on the listing. */
export function Coupons({ onChanged }: { onChanged: (message: string) => void }) {
  const resource = useNodeResource(fetchCoupons, 0);
  const [form, setForm] = useState<CouponForm>(EMPTY);
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const set = <K extends keyof CouponForm>(key: K, value: CouponForm[K]) => setForm(current => ({ ...current, [key]: value }));
  async function save(e: FormEvent) {
    e.preventDefault();
    const body = couponEditorDraft(form);
    if (typeof body === "string") {
      setError(body);
      return;
    }
    setError("");
    await run(async () => {
      await saveCoupon(body);
      setForm(EMPTY);
      await resource.refresh();
      onChanged("Coupon created");
    });
  }
  const coupons = resource.data ?? [];
  return <section className="ob-store-listings">
    <PageHeading title="Coupons" description="Manage discount codes, order minimums, validity dates and redemption limits."/>
    {(error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    <form className="ob-store-section" onSubmit={save}>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Code</span><input value={form.code} maxLength={24} autoCapitalize="characters" placeholder="FALL10" onChange={e => set("code", e.target.value)}/></label>
        <label className="ob-field"><span>Discount type</span><select value={form.kind} onChange={e => set("kind", e.target.value as CouponForm["kind"])}><option value="percent">Percentage</option><option value="fixed">Fixed amount</option></select></label>
        <label className="ob-field"><span>{form.kind === "percent" ? "Discount (%)" : `Discount (${form.currency})`}</span><input inputMode="decimal" value={form.amount} placeholder={form.kind === "percent" ? "10" : form.currency === "JPY" ? "500" : "5.00"} onChange={e => set("amount", e.target.value)}/><small className="ob-field-hint">{form.kind === "percent" ? "0.01% to 100%." : "Discount applied in the selected currency."}</small></label>
        <label className="ob-field"><span>Currency</span><select value={form.currency} onChange={e => set("currency", e.target.value)}>{['USD','EUR','GBP','JPY'].map(currency => <option key={currency}>{currency}</option>)}</select></label>
        <label className="ob-field"><span>Starts</span><input type="date" value={form.startsOn} onChange={e => set("startsOn", e.target.value)}/></label>
        <label className="ob-field"><span>Ends</span><input type="date" value={form.endsOn} onChange={e => set("endsOn", e.target.value)}/></label>
        <label className="ob-field"><span>Minimum order ({form.currency})</span><input inputMode="decimal" value={form.minSubtotal} placeholder="No minimum" onChange={e => set("minSubtotal", e.target.value)}/></label>
        <label className="ob-field"><span>Redemption limit</span><input inputMode="numeric" value={form.maxUses} placeholder="Unlimited" onChange={e => set("maxUses", e.target.value)}/></label>
      </div>
      <div className="ob-form-actions"><button className="ob-primary" type="submit">Create coupon</button></div>
    </form>
    {resource.loading ? <p role="status">Loading coupons…</p> : coupons.length ? <div className="ob-coupon-list"><table className="ob-merchant-table"><thead><tr><th>Code</th><th>Discount</th><th>Validity</th><th>Redemptions</th></tr></thead><tbody>{coupons.map(coupon => <tr key={coupon.code}>
      <td data-label="Code"><strong>{coupon.code}</strong></td>
      <td data-label="Discount">{coupon.percentBps != null ? `${coupon.percentBps / 100}%` : formatPrice(coupon.fixed ?? 0, coupon.currency as PriceCurrency)} off</td>
      <td data-label="Validity">{dateLabel(coupon.startsOn)} – {dateLabel(coupon.endsOn)}</td>
      <td data-label="Redemptions">{coupon.uses}{coupon.maxUses ? ` / ${coupon.maxUses}` : ' · unlimited'}</td>
    </tr>)}</tbody></table></div> : <p className="ob-shipping-note">No coupons yet.</p>}
    {dialog}
  </section>;
}
