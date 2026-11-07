import { useState } from "react";
import type { Axis } from "../commerce";
import { MAX_AXES, axesProblem, parseValues, syncRows, type VariantRow } from "../commerceDraft";
import { formatMinor, fromMinor, toMinor } from "../pricing";
import { useListingStyles } from "./VariantPicker";

type Props = { axes: Axis[]; rows: VariantRow[]; currency: string; basePrice: number; images: string[]; onChange: (axes: Axis[], rows: VariantRow[]) => void };

/// Option axes (Size, Color…) and the grid of combinations they make. Each
/// row has its own SKU, price, stock and image; a row switched off is not
/// offered and the buyer's picker greys it out.
export function VariantEditor({ axes, rows, currency, basePrice, images, onChange }: Props) {
  useListingStyles();
  const [drafts, setDrafts] = useState(() => axes.map(axis => axis.values.join(", ")));
  const setAxes = (next: Axis[], texts = drafts) => {
    setDrafts(texts);
    onChange(next, syncRows(next.filter(a => a.values.length), rows));
  };
  const setRow = (i: number, change: Partial<VariantRow>) => onChange(axes, rows.map((row, j) => (j === i ? { ...row, ...change } : row)));
  const problem = axesProblem(axes, rows);
  const offered = rows.filter(r => r.offered);
  return (
    <section className="ob-variant-editor" aria-label="Options and variants">
      <header>
        <h3>Options</h3>
        <p>Add up to {MAX_AXES} options, like size, color or material. Every combination gets a row below.</p>
      </header>
      {axes.map((axis, a) => (
        <div className="ob-axis-row" key={a}>
          <label className="ob-field">Option name
            <input value={axis.name} placeholder="Size" onChange={e => setAxes(axes.map((x, b) => (b === a ? { ...x, name: e.target.value } : x)))}/>
          </label>
          <label className="ob-field">Values, separated by commas
            <input
              value={drafts[a] ?? ""}
              placeholder="S, M, L"
              onChange={e => {
                const texts = drafts.map((t, b) => (b === a ? e.target.value : t));
                setAxes(axes.map((x, b) => (b === a ? { ...x, values: parseValues(e.target.value) } : x)), texts);
              }}
            />
          </label>
          <button type="button" className="ob-text-link" onClick={() => setAxes(axes.filter((_, b) => b !== a), drafts.filter((_, b) => b !== a))}>Remove</button>
        </div>
      ))}
      {axes.length < MAX_AXES && (
        <button type="button" className="ob-secondary ob-add-axis" onClick={() => setAxes([...axes, { name: "", values: [] }], [...drafts, ""])}>
          {axes.length ? "Add another option" : "Add options like size or color"}
        </button>
      )}
      {rows.length > 0 && (
        <div className="ob-variant-grid" role="table" aria-label="Variants">
          <div className="ob-variant-head" role="row">
            <span role="columnheader">Offer</span>
            <span role="columnheader">Variant</span>
            <span role="columnheader">SKU</span>
            <span role="columnheader">Price ({currency})</span>
            <span role="columnheader">Stock</span>
            <span role="columnheader">Photo</span>
          </div>
          {rows.map((row, i) => {
            const label = row.options.join(" / ");
            return (
              <div className={`ob-variant-row ${row.offered ? "" : "is-off"}`} role="row" key={label}>
                <span role="cell"><input type="checkbox" aria-label={`Offer ${label}`} checked={row.offered} onChange={e => setRow(i, { offered: e.target.checked })}/></span>
                <span role="cell" className="ob-variant-name">{label}</span>
                <span role="cell"><input aria-label={`SKU for ${label}`} value={row.sku} disabled={!row.offered} onChange={e => setRow(i, { sku: e.target.value })}/></span>
                <span role="cell">
                  <input
                    aria-label={`Price for ${label}`}
                    inputMode="decimal"
                    placeholder={fromMinor(basePrice, currency)}
                    defaultValue={row.price ? fromMinor(row.price, currency) : ""}
                    disabled={!row.offered}
                    onChange={e => setRow(i, { price: e.target.value.trim() ? toMinor(e.target.value, currency) ?? row.price : 0 })}
                  />
                </span>
                <span role="cell"><input aria-label={`Stock for ${label}`} inputMode="numeric" value={row.stock} disabled={!row.offered} onChange={e => setRow(i, { stock: Math.max(0, Math.min(1_000_000, Number(e.target.value.replace(/\D/g, "")) || 0)) })}/></span>
                <span role="cell">
                  <select aria-label={`Photo for ${label}`} value={row.image} disabled={!row.offered} onChange={e => setRow(i, { image: Number(e.target.value) })}>
                    <option value={0}>Cover</option>
                    {images.map((_, n) => <option key={n} value={n + 1}>Photo {n + 1}</option>)}
                  </select>
                </span>
              </div>
            );
          })}
          <p className="ob-variant-summary">
            {offered.length} of {rows.length} combinations offered · {offered.reduce((n, r) => n + r.stock, 0)} in stock · empty price uses {formatMinor(basePrice, currency)}
          </p>
        </div>
      )}
      {problem && <p className="ob-form-error" role="alert">{problem}</p>}
    </section>
  );
}
