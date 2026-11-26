import { useState, type FormEvent } from "react";
import { couponEditorDraft, type CouponForm } from "../couponDraft";
import { fetchCoupons, saveCoupon, formatPrice, type PriceCurrency } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { PageHeading } from "./PageHeading";
import { useLockedAction } from "./SellerKit";
import { Dialog } from "./WalletLock";

function formatIsoDate(date: Date) {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function emptyForm(): CouponForm {
  const start = new Date();
  const end = new Date(start.getFullYear(), start.getMonth(), start.getDate() + 30);
  return { code: "", kind: "percent", amount: "", currency: "USD", minSubtotal: "", startsOn: formatIsoDate(start), endsOn: formatIsoDate(end), maxUses: "", listings: [] };
}

const CURRENCIES = ["USD", "EUR", "GBP", "JPY"] as const;

function dateLabel(day: number) {
  const text = String(day);
  if (!/^\d{8}$/.test(text)) return '—';
  return new Date(Number(text.slice(0,4)), Number(text.slice(4,6)) - 1, Number(text.slice(6,8))).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
}

function TicketIcon() {
  return (
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M3 8.5A1.5 1.5 0 0 1 4.5 7h15A1.5 1.5 0 0 1 21 8.5V10a2 2 0 0 0 0 4v1.5a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 15.5V14a2 2 0 0 0 0-4V8.5Z" />
      <path d="M9 7v10" strokeDasharray="2.5 2.5" />
    </svg>
  );
}

/** Codes the store already signs. A coupon is not a field on the listing. */
export function Coupons({ onChanged }: { onChanged: (message: string) => void }) {
  const resource = useNodeResource(fetchCoupons, 0);
  const [open, setOpen] = useState(false);
  const [form, setForm] = useState<CouponForm>(emptyForm);
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const set = <K extends keyof CouponForm>(key: K, value: CouponForm[K]) => setForm(current => ({ ...current, [key]: value }));

  function openModal() {
    setError("");
    setOpen(true);
  }

  function closeModal() {
    setOpen(false);
    setError("");
  }

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
      setForm(emptyForm());
      setOpen(false);
      await resource.refresh();
      onChanged("Coupon created");
    });
  }

  const coupons = resource.data ?? [];
  return <section className="ob-store-listings ob-coupons">
    <PageHeading
      title="Coupons"
      description="Manage discount codes, order minimums, validity dates, and redemption limits."
      action={<button type="button" className="ob-primary" onClick={openModal}>+ New coupon</button>}
    />
    {!open && (error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    {resource.loading ? <p role="status">Loading coupons…</p> : coupons.length ? <div className="ob-coupon-list"><table className="ob-merchant-table"><thead><tr><th>Code</th><th>Discount</th><th>Validity</th><th>Redemptions</th></tr></thead><tbody>{coupons.map(coupon => <tr key={coupon.code}>
      <td data-label="Code"><strong>{coupon.code}</strong></td>
      <td data-label="Discount">{coupon.percentBps != null ? `${coupon.percentBps / 100}%` : formatPrice(coupon.fixed ?? 0, coupon.currency as PriceCurrency)} off</td>
      <td data-label="Validity">{dateLabel(coupon.startsOn)} – {dateLabel(coupon.endsOn)}</td>
      <td data-label="Redemptions">{coupon.uses}{coupon.maxUses ? ` / ${coupon.maxUses}` : ' · unlimited'}</td>
    </tr>)}</tbody></table></div> : <div className="ob-empty">
      <div className="ob-empty-icon"><TicketIcon /></div>
      <h2>No coupons yet</h2>
      <p>Create a discount code to offer percentage or fixed-amount savings at checkout.</p>
      <div className="ob-empty-actions">
        <button type="button" className="ob-primary" onClick={openModal}>+ New coupon</button>
      </div>
    </div>}
    {open && <Dialog label="New coupon" className="ob-coupon-dialog" onCancel={closeModal}>
      <form className="ob-coupon-form" onSubmit={save}>
        <h2>New coupon</h2>
        {error && <p className="ob-form-error" role="alert">{error}</p>}
        <div className="ob-coupon-row">
          <label className="ob-field">
            <span>Code</span>
            <input value={form.code} maxLength={24} autoCapitalize="characters" placeholder="FALL10" onChange={e => set("code", e.target.value)} data-dialog-autofocus/>
          </label>
          <label className="ob-field">
            <span>Discount type</span>
            <select value={form.kind} onChange={e => set("kind", e.target.value as CouponForm["kind"])}>
              <option value="percent">Percentage</option>
              <option value="fixed">Fixed amount</option>
            </select>
          </label>
        </div>
        <div className={form.kind === "fixed" ? "ob-coupon-row" : undefined}>
          <label className="ob-field">
            <span>{form.kind === "percent" ? "Discount (%)" : `Discount (${form.currency})`}</span>
            <input inputMode="decimal" value={form.amount} placeholder={form.kind === "percent" ? "10" : form.currency === "JPY" ? "500" : "5.00"} onChange={e => set("amount", e.target.value)}/>
            <small className="ob-field-hint">{form.kind === "percent" ? "0.01% to 100%." : "Discount applied in the selected currency."}</small>
          </label>
          {form.kind === "fixed" && <label className="ob-field">
            <span>Currency</span>
            <select value={form.currency} onChange={e => set("currency", e.target.value)}>
              {CURRENCIES.map(currency => <option key={currency}>{currency}</option>)}
            </select>
          </label>}
        </div>
        <div className="ob-coupon-row">
          <label className="ob-field">
            <span>Starts</span>
            <input type="date" value={form.startsOn} onChange={e => set("startsOn", e.target.value)}/>
          </label>
          <label className="ob-field">
            <span>Ends</span>
            <input type="date" value={form.endsOn} onChange={e => set("endsOn", e.target.value)}/>
          </label>
        </div>
        <details className="ob-coupon-optional">
          <summary>Limits (optional)</summary>
          <div className="ob-coupon-optional-body">
            <div className="ob-coupon-row">
              <label className="ob-field">
                <span>Minimum order ({form.currency})</span>
                <input inputMode="decimal" value={form.minSubtotal} placeholder="No minimum" onChange={e => set("minSubtotal", e.target.value)}/>
              </label>
              <label className="ob-field">
                <span>Redemption limit</span>
                <input inputMode="numeric" value={form.maxUses} placeholder="Unlimited" onChange={e => set("maxUses", e.target.value)}/>
              </label>
            </div>
            {form.kind === "percent" && <label className="ob-field">
              <span>Currency (for minimum order)</span>
              <select value={form.currency} onChange={e => set("currency", e.target.value)}>
                {CURRENCIES.map(currency => <option key={currency}>{currency}</option>)}
              </select>
            </label>}
          </div>
        </details>
        <div className="ob-form-actions">
          <button type="button" className="ob-secondary" onClick={closeModal}>Cancel</button>
          <button type="submit" className="ob-primary">Create coupon</button>
        </div>
      </form>
    </Dialog>}
    {dialog}
  </section>;
}
