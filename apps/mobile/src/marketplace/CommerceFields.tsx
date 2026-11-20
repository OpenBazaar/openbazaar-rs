import { useState } from "react";
import { CONDITION_LABELS, type Commerce, type Condition, type Service } from "../commerce";
import { defaultService, rowsOf, withRows, type PackageInput, type ReturnsDraft, type SpecificRow, type VariantRow } from "../commerceDraft";
import { SERVICE_NAMES, SPECIFIC_KEYS } from "../listingFields";
import { fromMinor, toMinor } from "../pricing";
import { ChoiceOrOther, CountryPicker, Field, WholeNumber, useRowIds } from "./EditorFields";
import { VariantEditor } from "./VariantEditor";

export type PackageFields = Omit<PackageInput, "regions">;

/// Most shipping services one listing signs: `MAX_SERVICES` in crates/protocol/src/shipping.rs.
const MAX_SERVICES = 6;
/// Most countries one service names: `MAX_SERVICE_REGIONS` in the same file.
const MAX_SERVICE_REGIONS = 32;
const MAX_SPECIFICS = 24;

type Change = (commerce: Commerce) => void;

/// Condition and the item specifics buyers filter and compare by.
export function ItemDetails({ commerce, onChange, specifics, onSpecifics }: { commerce: Commerce; onChange: Change; specifics: SpecificRow[]; onSpecifics: (rows: SpecificRow[]) => void }) {
  const ids = useRowIds(specifics.length);
  const setRow = (index: number, patch: Partial<SpecificRow>) => onSpecifics(specifics.map((row, at) => at === index ? { ...row, ...patch } : row));
  return <>
    <div className="ob-field-grid">
      <Field label="Condition"><select value={commerce.condition} onChange={e => onChange({ ...commerce, condition: e.target.value as Condition, conditionNote: e.target.value === "new" ? "" : commerce.conditionNote })}>{Object.entries(CONDITION_LABELS).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></Field>
      {commerce.condition !== "new" && <Field label="Condition note" hint="Optional. Wear, repairs or missing parts a buyer should know about."><input value={commerce.conditionNote} maxLength={200} onChange={e => onChange({ ...commerce, conditionNote: e.target.value })}/></Field>}
    </div>
    <div className="ob-editor-subsection">
      <h3>Item specifics</h3>
      <p className="ob-field-hint">Facts buyers compare by, such as brand or material. Product codes are checked when you save.</p>
      {specifics.map((row, index) => <div className="ob-service-row" key={ids.key(index)}>
        <ChoiceOrOther label="Specific" value={row.key} options={SPECIFIC_KEYS} maxLength={32} onChange={key => setRow(index, { key })}/>
        <Field label="Value"><input value={row.value} maxLength={120} onChange={e => setRow(index, { value: e.target.value })}/></Field>
        <button type="button" className="ob-text-link" aria-label={`Remove specific ${index + 1}`} onClick={() => { ids.remove(index); onSpecifics(specifics.filter((_, at) => at !== index)); }}>Remove</button>
      </div>)}
      {specifics.length < MAX_SPECIFICS && <button type="button" className="ob-secondary" onClick={() => { ids.add(); onSpecifics([...specifics, { key: "", value: "" }]); }}>Add an item specific</button>}
    </div>
  </>;
}

/// Option axes and the variants they make. The rows live here so a row the
/// seller switched off is still listed while they edit.
export function OptionFields({ commerce, onChange, currency, basePrice, images, variantWeights, onVariantWeights }: { commerce: Commerce; onChange: Change; currency: string; basePrice: number; images: string[]; variantWeights: string[]; onVariantWeights: (weights: string[]) => void }) {
  const [rows, setRows] = useState<VariantRow[]>(() => rowsOf(commerce).map((row, index) => ({ ...row, weight: variantWeights[index] ?? (row.weightG ? String(row.weightG) : "") })));
  return <VariantEditor axes={commerce.axes} rows={rows} currency={currency} basePrice={basePrice} images={images} onChange={(axes, next) => { setRows(next); onVariantWeights(next.map(row => row.weight ?? "")); onChange(withRows(commerce, axes, next)); }}/>;
}

/// Shipping services, the package they carry, and local pickup.
export function ShippingServices({ commerce, onChange, currency, packageFields, onPackage }: { commerce: Commerce; onChange: Change; currency: string; packageFields: PackageFields; onPackage: (patch: Partial<PackageFields>) => void }) {
  const shipping = commerce.shipping;
  const ids = useRowIds(shipping.services.length);
  const setShipping = (patch: Partial<Commerce["shipping"]>) => onChange({ ...commerce, shipping: { ...shipping, ...patch } });
  const setService = (i: number, patch: Partial<Service>) => setShipping({ services: shipping.services.map((s, n) => n === i ? { ...s, ...patch } : s) });
  const money = (value: string, fallback: number) => value.trim() ? toMinor(value, currency) ?? fallback : 0;
  const days = (value: string) => Math.min(Number(value) || 0, 60);
  const unused = SERVICE_NAMES.find(name => !shipping.services.some(s => s.name.toLowerCase() === name.toLowerCase())) ?? "";
  return <>
    <div className="ob-editor-subsection">
      <h3>Shipping services</h3>
      <p className="ob-field-hint">Give buyers a choice of speed and price, and say which countries each one reaches. With no services, the flat shipping price above applies everywhere.</p>
      {shipping.services.map((s, i) => <div className="ob-service-row ob-shipping-service" key={ids.key(i)}>
        <ChoiceOrOther label="Service" value={s.name} options={SERVICE_NAMES} maxLength={40} onChange={name => setService(i, { name })}/>
        <Field label={`Price (${currency})`}><input inputMode="decimal" defaultValue={s.price ? fromMinor(s.price, currency) : ""} placeholder="Free" onChange={e => setService(i, { price: money(e.target.value, s.price) })}/></Field>
        <Field label={`Each extra item (${currency})`} hint="Added for every further item in the same package."><input inputMode="decimal" defaultValue={s.extraItem ? fromMinor(s.extraItem, currency) : ""} placeholder="Free" onChange={e => setService(i, { extraItem: money(e.target.value, s.extraItem) })}/></Field>
        <div className="ob-field ob-day-range" role="group" aria-label="Delivery time in days">
          <span>Delivery time (days)</span>
          <div><WholeNumber label="Fastest (days)" value={String(s.daysMin)} max={60} onChange={v => setService(i, { daysMin: days(v) })}/><span aria-hidden="true">to</span><WholeNumber label="Slowest (days)" value={String(s.daysMax)} max={60} onChange={v => setService(i, { daysMax: days(v) })}/></div>
        </div>
        <CountryPicker label="Ships to" codes={s.regions} max={MAX_SERVICE_REGIONS} onChange={regions => setService(i, { regions })}/>
        <button type="button" className="ob-text-link" aria-label={`Remove ${s.name || "this service"}`} onClick={() => { ids.remove(i); setShipping({ services: shipping.services.filter((_, n) => n !== i) }); }}>Remove service</button>
      </div>)}
      {shipping.services.length < MAX_SERVICES && <button type="button" className="ob-secondary" onClick={() => { ids.add(); setShipping({ services: [...shipping.services, { ...defaultService(), name: unused }] }); }}>Add a shipping service</button>}
    </div>
    <div className="ob-editor-subsection">
      <h3>Package and handling</h3>
      <div className="ob-field-grid ob-field-grid-4">
        <Field label="Weight (g)"><WholeNumber value={packageFields.weightG} onChange={weightG => onPackage({ weightG })}/></Field>
        <Field label="Length (mm)"><WholeNumber value={packageFields.lengthMm} max={65535} onChange={lengthMm => onPackage({ lengthMm })}/></Field>
        <Field label="Width (mm)"><WholeNumber value={packageFields.widthMm} max={65535} onChange={widthMm => onPackage({ widthMm })}/></Field>
        <Field label="Height (mm)"><WholeNumber value={packageFields.heightMm} max={65535} onChange={heightMm => onPackage({ heightMm })}/></Field>
      </div>
      <div className="ob-field-grid">
        <Field label="Handling time (days)" hint="Days before the package leaves you. At most 30."><WholeNumber value={packageFields.handlingDays} max={30} onChange={handlingDays => onPackage({ handlingDays })}/></Field>
        <Field label={`Free shipping over (${currency})`} hint="Optional. Orders above this ship free."><input inputMode="decimal" value={packageFields.freeOver} onChange={e => onPackage({ freeOver: e.target.value })}/></Field>
      </div>
    </div>
    <div className="ob-editor-subsection">
      <h3>Local pickup</h3>
      <Field label="Pickup area" hint="Optional. A city, state or country only, never an address. Buyers who choose pickup arrange the meeting with you in private messages."><input value={shipping.pickup ?? ""} maxLength={40} onChange={e => setShipping({ pickup: e.target.value || null })}/></Field>
    </div>
  </>;
}

/// The returns terms this listing signs in place of the store's policy.
export function ReturnsFields({ returns, onReturns }: { returns: ReturnsDraft; onReturns: (patch: Partial<ReturnsDraft>) => void }) {
  return <div className="ob-field-grid">
    <Field label="Returns"><select value={returns.kind} onChange={e => onReturns({ kind: e.target.value as ReturnsDraft["kind"] })}><option value="store">Follow the store policy</option><option value="none">No returns</option><option value="days">Accept returns for a set number of days</option></select></Field>
    {returns.kind === "days" && <>
      <Field label="Return window (days)" hint="From 1 to 365."><WholeNumber value={returns.days} min={1} max={365} onChange={days => onReturns({ days })}/></Field>
      <label className="ob-default-address"><input type="checkbox" checked={returns.sellerPays} onChange={e => onReturns({ sellerPays: e.target.checked })}/>You pay return shipping</label>
    </>}
  </div>;
}
