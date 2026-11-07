import { useState } from "react";
import { CONDITION_LABELS, type Commerce, type Condition, type Service } from "../commerce";
import { defaultService, rowsOf, withRows, type VariantRow } from "../commerceDraft";
import { fromMinor, toMinor } from "../pricing";
import { VariantEditor } from "./VariantEditor";

type Props = { commerce: Commerce; kind: "physical" | "digital"; currency: string; basePrice: number; images: string[]; onChange: (commerce: Commerce) => void };

const MAX_SERVICES = 8;

/// Condition, options, per-order limit and shipping services for a version 7 listing.
export function CommerceFields({ commerce, kind, currency, basePrice, images, onChange }: Props) {
  const [rows, setRows] = useState<VariantRow[]>(() => rowsOf(commerce));
  const set = (patch: Partial<Commerce>) => onChange({ ...commerce, ...patch });
  const shipping = commerce.shipping;
  const setService = (i: number, patch: Partial<Service>) => set({ shipping: { ...shipping, services: shipping.services.map((s, n) => n === i ? { ...s, ...patch } : s) } });
  const money = (value: string, fallback: number) => value.trim() ? toMinor(value, currency) ?? fallback : 0;
  return <>
    <fieldset className="ob-store-section">
      <legend>Condition and options</legend>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Condition</span><select value={commerce.condition} onChange={e => set({ condition: e.target.value as Condition })}>{Object.entries(CONDITION_LABELS).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <label className="ob-field"><span>Most per order</span><input inputMode="numeric" value={commerce.maxPerOrder || ""} placeholder="No limit" onChange={e => set({ maxPerOrder: Math.min(999, Number(e.target.value.replace(/\D/g, "")) || 0) })}/></label>
        {commerce.condition !== "new" && <label className="ob-field ob-field-wide"><span>Condition note</span><input value={commerce.conditionNote} maxLength={200} placeholder="Light scuff on the lid" onChange={e => set({ conditionNote: e.target.value })}/></label>}
      </div>
      <VariantEditor axes={commerce.axes} rows={rows} currency={currency} basePrice={basePrice} images={images} onChange={(axes, next) => { setRows(next); onChange(withRows(commerce, axes, next)); }}/>
    </fieldset>
    {kind === "physical" && <fieldset className="ob-store-section">
      <legend>Shipping services</legend>
      {shipping.services.map((s, i) => <div className="ob-service-row" key={i}>
        <label className="ob-field"><span>Service</span><input value={s.name} maxLength={40} onChange={e => setService(i, { name: e.target.value })}/></label>
        <label className="ob-field"><span>Price ({currency})</span><input inputMode="decimal" defaultValue={s.price ? fromMinor(s.price, currency) : ""} placeholder="Free" onChange={e => setService(i, { price: money(e.target.value, s.price) })}/></label>
        <label className="ob-field"><span>Each extra item</span><input inputMode="decimal" defaultValue={s.extraItem ? fromMinor(s.extraItem, currency) : ""} placeholder="0.00" onChange={e => setService(i, { extraItem: money(e.target.value, s.extraItem) })}/></label>
        <label className="ob-field"><span>Fastest (days)</span><input inputMode="numeric" value={s.daysMin} onChange={e => setService(i, { daysMin: Math.min(Number(e.target.value.replace(/\D/g, "")) || 0, 60) })}/></label>
        <label className="ob-field"><span>Slowest (days)</span><input inputMode="numeric" value={s.daysMax} onChange={e => setService(i, { daysMax: Math.min(Number(e.target.value.replace(/\D/g, "")) || 0, 60) })}/></label>
        <button type="button" className="ob-text-link" aria-label={`Remove ${s.name || "this service"}`} onClick={() => set({ shipping: { ...shipping, services: shipping.services.filter((_, n) => n !== i) } })}>Remove</button>
      </div>)}
      {shipping.services.length < MAX_SERVICES && <button type="button" className="ob-secondary" onClick={() => set({ shipping: { ...shipping, services: [...shipping.services, defaultService()] } })}>Add a shipping service</button>}
      <label className="ob-field"><span>Local pickup area (optional)</span><input value={shipping.pickup ?? ""} maxLength={40} placeholder="Portland" onChange={e => set({ shipping: { ...shipping, pickup: e.target.value || null } })}/><small className="ob-field-hint">Name a city, state or country only, with no commas or numbers, never an address. Buyers who choose pickup arrange the meeting with you in private messages.</small></label>
    </fieldset>}
  </>;
}
