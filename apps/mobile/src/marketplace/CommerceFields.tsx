import { useState } from "react";
import { CONDITION_LABELS, type Commerce, type Condition, type Service } from "../commerce";
import { defaultService, rowsOf, withRows, type PackageInput, type ReturnsDraft, type SpecificRow, type VariantRow } from "../commerceDraft";
import { fromMinor, toMinor } from "../pricing";
import { VariantEditor } from "./VariantEditor";

type PackageFields = Omit<PackageInput, "regions">;
type Props = {
  commerce: Commerce;
  kind: "physical" | "digital";
  currency: string;
  basePrice: number;
  images: string[];
  packageFields: PackageFields;
  onPackage: (patch: Partial<PackageFields>) => void;
  specifics: SpecificRow[];
  onSpecifics: (rows: SpecificRow[]) => void;
  returns: ReturnsDraft;
  onReturns: (patch: Partial<ReturnsDraft>) => void;
  onChange: (commerce: Commerce) => void;
};

const MAX_SERVICES = 8;

/// Condition, options, per-order limit and shipping services for a version 7 listing.
export function CommerceFields({ commerce, kind, currency, basePrice, images, packageFields, onPackage, specifics, onSpecifics, returns, onReturns, onChange }: Props) {
  const [rows, setRows] = useState<VariantRow[]>(() => rowsOf(commerce));
  const [regionDrafts, setRegionDrafts] = useState(() => commerce.shipping.services.map(service => service.regions.join(", ")));
  const set = (patch: Partial<Commerce>) => onChange({ ...commerce, ...patch });
  const shipping = commerce.shipping;
  const setService = (i: number, patch: Partial<Service>) => set({ shipping: { ...shipping, services: shipping.services.map((s, n) => n === i ? { ...s, ...patch } : s) } });
  const setRegions = (i: number, text: string) => {
    const drafts = regionDrafts.map((draft, n) => n === i ? text : draft);
    setRegionDrafts(drafts);
    setService(i, { regions: text.split(/[\s,]+/).map(code => code.trim().toUpperCase()).filter(Boolean) });
  };
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
      <div className="ob-field-grid">
        {specifics.map((row, index) => <div className="ob-service-row" key={index}>
          <label className="ob-field"><span>Specific</span><input value={row.key} maxLength={32} placeholder="Brand" onChange={e => onSpecifics(specifics.map((item, at) => at === index ? { ...item, key: e.target.value } : item))}/></label>
          <label className="ob-field"><span>Value</span><input value={row.value} maxLength={120} placeholder="Marlow" onChange={e => onSpecifics(specifics.map((item, at) => at === index ? { ...item, value: e.target.value } : item))}/></label>
          <button type="button" className="ob-text-link" aria-label={`Remove specific ${index + 1}`} onClick={() => onSpecifics(specifics.filter((_, at) => at !== index))}>Remove</button>
        </div>)}
        {specifics.length < 24 && <button type="button" className="ob-secondary" onClick={() => onSpecifics([...specifics, { key: "", value: "" }])}>Add an item specific</button>}
        <label className="ob-field"><span>Returns</span><select value={returns.kind} onChange={e => onReturns({ kind: e.target.value as ReturnsDraft["kind"] })}><option value="store">Store policy</option><option value="none">No returns</option><option value="days">A set number of days</option></select></label>
        {returns.kind === "days" && <>
          <label className="ob-field"><span>Return window (days)</span><input inputMode="numeric" value={returns.days} placeholder="30" onChange={e => onReturns({ days: e.target.value })}/></label>
          <label className="ob-default-address"><input type="checkbox" checked={returns.sellerPays} onChange={e => onReturns({ sellerPays: e.target.checked })}/>Seller pays return shipping</label>
        </>}
      </div>
    </fieldset>
    {kind === "physical" && <fieldset className="ob-store-section">
      <legend>Shipping services</legend>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Package weight (g)</span><input inputMode="numeric" value={packageFields.weightG} placeholder="0" onChange={e => onPackage({ weightG: e.target.value })}/></label>
        <label className="ob-field"><span>Length (mm)</span><input inputMode="numeric" value={packageFields.lengthMm} placeholder="0" onChange={e => onPackage({ lengthMm: e.target.value })}/></label>
        <label className="ob-field"><span>Width (mm)</span><input inputMode="numeric" value={packageFields.widthMm} placeholder="0" onChange={e => onPackage({ widthMm: e.target.value })}/></label>
        <label className="ob-field"><span>Height (mm)</span><input inputMode="numeric" value={packageFields.heightMm} placeholder="0" onChange={e => onPackage({ heightMm: e.target.value })}/></label>
        <label className="ob-field"><span>Handling time (days)</span><input inputMode="numeric" value={packageFields.handlingDays} placeholder="0" onChange={e => onPackage({ handlingDays: e.target.value })}/></label>
        <label className="ob-field"><span>Free shipping over ({currency})</span><input inputMode="decimal" value={packageFields.freeOver} placeholder="None" onChange={e => onPackage({ freeOver: e.target.value })}/></label>
      </div>
      {shipping.services.map((s, i) => <div className="ob-service-row" key={i}>
        <label className="ob-field"><span>Service</span><input value={s.name} maxLength={40} onChange={e => setService(i, { name: e.target.value })}/></label>
        <label className="ob-field"><span>Price ({currency})</span><input inputMode="decimal" defaultValue={s.price ? fromMinor(s.price, currency) : ""} placeholder="Free" onChange={e => setService(i, { price: money(e.target.value, s.price) })}/></label>
        <label className="ob-field"><span>Each extra item</span><input inputMode="decimal" defaultValue={s.extraItem ? fromMinor(s.extraItem, currency) : ""} placeholder="0.00" onChange={e => setService(i, { extraItem: money(e.target.value, s.extraItem) })}/></label>
        <label className="ob-field"><span>Fastest (days)</span><input inputMode="numeric" value={s.daysMin} onChange={e => setService(i, { daysMin: Math.min(Number(e.target.value.replace(/\D/g, "")) || 0, 60) })}/></label>
        <label className="ob-field"><span>Slowest (days)</span><input inputMode="numeric" value={s.daysMax} onChange={e => setService(i, { daysMax: Math.min(Number(e.target.value.replace(/\D/g, "")) || 0, 60) })}/></label>
        <label className="ob-field"><span>Ships to</span><input value={regionDrafts[i] ?? ""} placeholder="Worldwide" onChange={e => setRegions(i, e.target.value)}/><small className="ob-field-hint">Country codes, like US, CA. Leave empty for worldwide.</small></label>
        <button type="button" className="ob-text-link" aria-label={`Remove ${s.name || "this service"}`} onClick={() => { setRegionDrafts(regionDrafts.filter((_, n) => n !== i)); set({ shipping: { ...shipping, services: shipping.services.filter((_, n) => n !== i) } }); }}>Remove</button>
      </div>)}
      {shipping.services.length < MAX_SERVICES && <button type="button" className="ob-secondary" onClick={() => { setRegionDrafts([...regionDrafts, ""]); set({ shipping: { ...shipping, services: [...shipping.services, defaultService()] } }); }}>Add a shipping service</button>}
      <label className="ob-field"><span>Local pickup area (optional)</span><input value={shipping.pickup ?? ""} maxLength={40} placeholder="Portland" onChange={e => set({ shipping: { ...shipping, pickup: e.target.value || null } })}/><small className="ob-field-hint">Name a city, state or country only, with no commas or numbers, never an address. Buyers who choose pickup arrange the meeting with you in private messages.</small></label>
    </fieldset>}
  </>;
}
