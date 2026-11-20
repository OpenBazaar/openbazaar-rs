import { cloneElement, useId, useRef, useState, type ReactElement, type ReactNode } from "react";
import { COUNTRIES, countryName, countryOf } from "../countries";
import { sameChoice } from "../listingFields";

/// Where the editor found a problem: the id of a field or a section, and what to fix.
export type Problem = { at: string; message: string };

/// A labelled field with its hint and, when the editor refused it, the reason.
/// The label points at the control instead of wrapping it, so the control's
/// name is the label alone and the hint or error is its description.
export function Field({ label, hint, error, wide = false, children }: { label: string; hint?: ReactNode; error?: string | null; wide?: boolean; children: ReactElement<{ id?: string; "aria-describedby"?: string }> }) {
  const generated = useId();
  const id = children.props.id ?? generated;
  const note = error || hint ? `${id}-note` : undefined;
  return <div className={`ob-field ${wide ? "ob-field-wide" : ""} ${error ? "has-error" : ""}`}>
    <label htmlFor={id}>{label}</label>
    {cloneElement(children, { id, "aria-describedby": note })}
    {error ? <small className="ob-field-error" id={note}>{error}</small> : hint ? <small className="ob-field-hint" id={note}>{hint}</small> : null}
  </div>;
}

/// A whole number typed with the number keypad and the browser's own range check.
export function WholeNumber({ id, value, onChange, min = 0, max, placeholder, label, invalid, "aria-describedby": describedBy }: { id?: string; value: string; onChange: (value: string) => void; min?: number; max?: number; placeholder?: string; label?: string; invalid?: boolean; "aria-describedby"?: string }) {
  return <input id={id} aria-describedby={describedBy} type="number" inputMode="numeric" step={1} min={min} max={max} value={value} placeholder={placeholder} aria-label={label} aria-invalid={invalid || undefined} onChange={e => onChange(e.target.value.replace(/[^\d]/g, ""))}/>;
}

/// A fixed list of choices with "Other" for the rare value the list lacks.
export function ChoiceOrOther({ label, value, options, onChange, maxLength, otherPlaceholder }: { label: string; value: string; options: readonly string[]; onChange: (value: string) => void; maxLength: number; otherPlaceholder?: string }) {
  const known = options.find(option => sameChoice(option, value)) ?? null;
  // "Other" was picked and nothing typed yet. A typed value the list lacks is "Other" on its own.
  const [blankOther, setBlankOther] = useState(false);
  const other = !known && (value.trim() !== "" || blankOther);
  const setOther = setBlankOther;
  const picked = other ? "\u0000other" : known ?? "";
  return <div className="ob-field ob-choice-or-other">
    <span>{label}</span>
    <select aria-label={label} value={picked} onChange={e => {
      if (e.target.value === "\u0000other") { setOther(true); onChange(""); return; }
      setOther(false);
      onChange(e.target.value);
    }}>
      <option value="" disabled>Choose…</option>
      {options.map(option => <option key={option} value={option}>{option}</option>)}
      <option value={"\u0000other"}>Other…</option>
    </select>
    {other && <input aria-label={`${label}, other`} value={value} maxLength={maxLength} placeholder={otherPlaceholder} autoFocus onChange={e => onChange(e.target.value)}/>}
  </div>;
}

/// One country, signed by name. A listing saved with other text keeps that
/// text as a choice until the seller picks a country.
export function CountrySelect({ id, label, value, onChange, hint, error }: { id?: string; label: string; value: string; onChange: (name: string) => void; hint?: ReactNode; error?: string | null }) {
  const country = countryOf(value);
  const legacy = value.trim() && !country ? value : null;
  return <Field label={label} hint={hint} error={error}>
    <select id={id} value={country?.name ?? legacy ?? ""} aria-invalid={error ? true : undefined} onChange={e => onChange(e.target.value)}>
      <option value="" disabled>Choose a country…</option>
      {legacy && <option value={legacy}>{legacy} (saved earlier)</option>}
      {COUNTRIES.map(c => <option key={c.code} value={c.name}>{c.name}</option>)}
    </select>
  </Field>;
}

/// The countries a shipping service reaches, as codes. None means worldwide.
export function CountryPicker({ label, codes, onChange, max }: { label: string; codes: string[]; onChange: (codes: string[]) => void; max: number }) {
  const hint = useId();
  return <div className="ob-field ob-country-picker">
    <span>{label}</span>
    <div className="ob-chip-row" role="list" aria-label={label}>
      {codes.length === 0 && <span className="ob-chip is-quiet" role="listitem">Worldwide</span>}
      {codes.map(code => <span className="ob-chip" role="listitem" key={code}>{countryName(code)}<button type="button" aria-label={`Remove ${countryName(code)}`} onClick={() => onChange(codes.filter(c => c !== code))}>×</button></span>)}
    </div>
    {codes.length < max && <select aria-label={`Add a country to ${label.toLowerCase()}`} aria-describedby={hint} value="" onChange={e => { if (e.target.value) onChange([...new Set([...codes, e.target.value])].sort()); }}>
      <option value="">{codes.length ? "Add another country…" : "Limit to a country…"}</option>
      {COUNTRIES.filter(c => !codes.includes(c.code)).map(c => <option key={c.code} value={c.code}>{c.name}</option>)}
    </select>}
    <small className="ob-field-hint" id={hint}>{codes.length ? `Up to ${max} countries. Remove them all to ship worldwide.` : "Ships everywhere until you pick a country."}</small>
  </div>;
}

/// A switch that reveals the fields under it. Turning it off clears them.
export function Reveal({ label, hint, on, onToggle, children }: { label: string; hint?: string; on: boolean; onToggle: (on: boolean) => void; children?: ReactNode }) {
  return <div className={`ob-reveal ${on ? "is-on" : ""}`}>
    <label className="ob-reveal-toggle"><input type="checkbox" checked={on} onChange={e => onToggle(e.target.checked)}/><span><strong>{label}</strong>{hint && <small>{hint}</small>}</span></label>
    {on && children && <div className="ob-reveal-body">{children}</div>}
  </div>;
}

/// Stable React keys for rows that are added and removed by position, so a
/// field that keeps its own text stays with its row when another is removed.
export function useRowIds(count: number) {
  const next = useRef(count);
  const [ids, setIds] = useState(() => Array.from({ length: count }, (_, index) => index));
  return {
    key: (index: number) => ids[index] ?? `late-${index}`,
    add: () => setIds(list => [...list, next.current++]),
    remove: (index: number) => setIds(list => list.filter((_, at) => at !== index)),
  };
}
