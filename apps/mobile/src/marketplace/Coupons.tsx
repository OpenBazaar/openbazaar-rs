import { useState, type FormEvent } from "react";
import { couponDraft, type CouponForm } from "../couponDraft";
import { fetchCoupons, saveCoupon } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { PageHeading } from "./PageHeading";
import { useLockedAction } from "./SellerKit";

const EMPTY: CouponForm = { code: "", kind: "percent", amount: "", currency: "USD", minSubtotal: "", startsOn: "", endsOn: "", maxUses: "", listings: [] };

/** Codes the store already signs. A coupon is not a field on the listing. */
export function Coupons({ onChanged }: { onChanged: (message: string) => void }) {
  const resource = useNodeResource(fetchCoupons, 0);
  const [form, setForm] = useState<CouponForm>(EMPTY);
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const set = <K extends keyof CouponForm>(key: K, value: CouponForm[K]) => setForm(current => ({ ...current, [key]: value }));
  async function save(e: FormEvent) {
    e.preventDefault();
    const body = couponDraft(form);
    if (typeof body === "string") {
      setError(body);
      return;
    }
    setError("");
    await run(async () => {
      await saveCoupon(body);
      setForm(EMPTY);
      await resource.refresh();
      onChanged("Coupon signed");
    });
  }
  const coupons = resource.data ?? [];
  return <section className="ob-store-listings">
    <PageHeading title="Coupons" description="A code the store signs. Buyers type it at checkout. It is not part of a listing."/>
    {(error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    <form className="ob-store-section" onSubmit={save}>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Code</span><input value={form.code} maxLength={24} autoCapitalize="characters" placeholder="FALL10" onChange={e => set("code", e.target.value)}/></label>
        <label className="ob-field"><span>Discount</span><select value={form.kind} onChange={e => set("kind", e.target.value as CouponForm["kind"])}><option value="percent">Percent, in basis points</option><option value="fixed">Fixed amount</option></select></label>
        <label className="ob-field"><span>{form.kind === "percent" ? "Basis points" : "Amount"}</span><input inputMode="numeric" value={form.amount} placeholder={form.kind === "percent" ? "1000" : "500"} onChange={e => set("amount", e.target.value)}/><small className="ob-field-hint">{form.kind === "percent" ? "1000 is 10%. From 1 to 10000." : "Minor units, at least 1."}</small></label>
        <label className="ob-field"><span>Currency</span><input value={form.currency} maxLength={3} onChange={e => set("currency", e.target.value)}/></label>
        <label className="ob-field"><span>Starts</span><input type="date" value={form.startsOn} onChange={e => set("startsOn", e.target.value)}/></label>
        <label className="ob-field"><span>Ends</span><input type="date" value={form.endsOn} onChange={e => set("endsOn", e.target.value)}/></label>
        <label className="ob-field"><span>Minimum</span><input inputMode="numeric" value={form.minSubtotal} placeholder="0" onChange={e => set("minSubtotal", e.target.value)}/></label>
        <label className="ob-field"><span>Use limit</span><input inputMode="numeric" value={form.maxUses} placeholder="Unlimited" onChange={e => set("maxUses", e.target.value)}/></label>
      </div>
      <button className="ob-primary" type="submit">Sign coupon</button>
    </form>
    {resource.loading ? <p role="status">Loading coupons…</p> : coupons.length ? <ul className="ob-own-listings">{coupons.map(coupon => <li key={coupon.code} className="ob-own-listing">
      <span className="ob-own-listing-copy">
        <strong>{coupon.code}</strong>
        <span>{coupon.percentBps != null ? `${coupon.percentBps / 100}%` : coupon.fixed} {coupon.currency} · {coupon.startsOn}–{coupon.endsOn}{coupon.maxUses ? ` · ${coupon.uses}/${coupon.maxUses}` : ""}</span>
      </span>
    </li>)}</ul> : <p className="ob-shipping-note">No coupons yet.</p>}
    {dialog}
  </section>;
}
