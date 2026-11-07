import { useEffect } from "react";
import { CONDITION_LABELS, type Commerce, type FiatPrice } from "../commerce";
import { formatMinor, off } from "../pricing";
import { choose, listingSellable, maxQuantity, optionStates, selectedVariant, sellable, stockNote, type Selection } from "../variants";

export function useListingStyles() {
  useEffect(() => {
    if (document.querySelector("link[data-ob-listings]")) return;
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = "listings.css";
    link.dataset.obListings = "";
    document.head.appendChild(link);
  }, []);
}

const STATE_LABEL = { available: "", "sold-out": " (sold out)", unavailable: " (not offered with this choice)" } as const;

/// Price block: the price today, the regular or compare-at price struck
/// through, and a sale badge. All in the listing's own currency.
export function PriceTag({ commerce, price, unit, today }: { commerce: Commerce | null; price: FiatPrice; unit: number; today: number }) {
  const sale = commerce?.sale && commerce.sale.startsOn <= today && today <= commerce.sale.endsOn ? commerce.sale : null;
  const regular = sale ? unit : 0;
  const shown = sale ? unit - off(unit, sale.offBps) : unit;
  const struck = sale ? regular : commerce?.compareAt && commerce.compareAt > shown ? commerce.compareAt : 0;
  return (
    <div className="ob-price-tag">
      <strong>{formatMinor(shown, price.currency)}</strong>
      {struck > 0 && <s aria-label={sale ? "Regular price" : "List price"}>{formatMinor(struck, price.currency)}</s>}
      {sale && <span className="ob-sale-badge">{sale.offBps / 100}% off until {String(sale.endsOn).replace(/(\d{4})(\d{2})(\d{2})/, "$2/$3")}</span>}
    </div>
  );
}

/// The buyer's choice of variant and quantity. Values that are not offered
/// with the other choices are greyed out; sold-out values are struck through.
export function VariantPicker({ commerce, stock = 0, selection, onSelect, quantity, onQuantity, asOf }: { commerce: Commerce; stock?: number; selection: Selection; onSelect: (s: Selection) => void; quantity: number; onQuantity: (n: number) => void; asOf: number }) {
  useListingStyles();
  const states = optionStates(commerce, selection);
  const variant = selectedVariant(commerce, selection);
  const chosen = variant !== null || commerce.axes.length === 0;
  const available = variant ? sellable(commerce, variant) : chosen ? listingSellable(commerce, stock) : 0;
  const note = chosen ? stockNote(available, commerce.availability, asOf) : null;
  const max = chosen ? maxQuantity(commerce, available) : 0;
  const tier = [...commerce.tiers].reverse().find(t => t.minQty <= quantity);
  const next = commerce.tiers.find(t => t.minQty > quantity && t.minQty <= max);
  return (
    <div className="ob-variant-picker">
      <span className="ob-condition-chip" title={commerce.conditionNote || undefined}>{CONDITION_LABELS[commerce.condition]}</span>
      {commerce.axes.map((axis, a) => (
        <fieldset className="ob-axis" key={axis.name}>
          <legend>{axis.name}{selection[a] !== null && <span>: {axis.values[selection[a]!]}</span>}</legend>
          <div className="ob-axis-values">
            {axis.values.map((value, v) => {
              const state = states[a][v];
              return (
                <button
                  key={value}
                  type="button"
                  className={`ob-axis-value is-${state}`}
                  aria-pressed={selection[a] === v}
                  aria-label={`${axis.name} ${value}${STATE_LABEL[state]}`}
                  disabled={state === "unavailable"}
                  onClick={() => onSelect(choose(commerce, selection, a, v))}
                >
                  {value}
                </button>
              );
            })}
          </div>
        </fieldset>
      ))}
      {commerce.axes.length > 0 && !variant && <p className="ob-stock-note is-info">Choose {commerce.axes.filter((_, a) => selection[a] === null).map(x => x.name.toLowerCase()).join(" and ")}.</p>}
      {note && <p className={`ob-stock-note is-${note.tone}`} role="status">{note.text}</p>}
      {chosen && max > 0 && (
        <div className="ob-buy-quantity">
          <span>Quantity</span>
          <div className="ob-stepper">
            <button type="button" aria-label="One fewer" disabled={quantity <= 1} onClick={() => onQuantity(quantity - 1)}>−</button>
            <output aria-live="polite">{quantity}</output>
            <button type="button" aria-label="One more" disabled={quantity >= max} onClick={() => onQuantity(quantity + 1)}>+</button>
          </div>
          {commerce.maxPerOrder > 0 && <small>Up to {commerce.maxPerOrder} per order</small>}
        </div>
      )}
      {(tier || next) && <p className="ob-tier-note">{tier ? `${tier.offBps / 100}% off for buying ${tier.minQty} or more` : ""}{tier && next ? " · " : ""}{next ? `Buy ${next.minQty}, save ${next.offBps / 100}%` : ""}</p>}
    </div>
  );
}

/// Condition, item specifics, shipping services, pickup and returns.
export function ListingFacts({ commerce, currency }: { commerce: Commerce; currency: string }) {
  useListingStyles();
  const specifics = Object.entries(commerce.specifics);
  const { shipping, returns } = commerce;
  return (
    <div className="ob-listing-facts">
      {(specifics.length > 0 || commerce.conditionNote) && (
        <dl className="ob-specifics">
          <div><dt>Condition</dt><dd>{CONDITION_LABELS[commerce.condition]}{commerce.conditionNote && <small>{commerce.conditionNote}</small>}</dd></div>
          {specifics.map(([key, value]) => <div key={key}><dt>{key.replace(/-/g, " ")}</dt><dd>{value}</dd></div>)}
          {commerce.lotSize > 1 && <div><dt>Lot</dt><dd>{commerce.lotSize} pieces per unit</dd></div>}
        </dl>
      )}
      {shipping.services.length > 0 && (
        <div className="ob-services">
          <h3>Shipping{shipping.handlingDays > 0 && <small> · ships within {shipping.handlingDays} {shipping.handlingDays === 1 ? "day" : "days"}</small>}</h3>
          {shipping.services.map(s => (
            <div className="ob-service" key={s.name}>
              <span><strong>{s.name}</strong><small>{s.daysMin}–{s.daysMax} days{s.regions.length ? ` · ${s.regions.join(", ")}` : " · worldwide"}</small></span>
              <span>{s.price ? formatMinor(s.price, currency) : "Free"}{s.extraItem > 0 && <small>+{formatMinor(s.extraItem, currency)} each extra</small>}</span>
            </div>
          ))}
          {shipping.freeOver > 0 && <p className="ob-shipping-note">Free shipping on orders from this seller over {formatMinor(shipping.freeOver, currency)}.</p>}
        </div>
      )}
      {shipping.pickup && <p className="ob-pickup-note">Local pickup available in {shipping.pickup}. You arrange the time and place with the seller in private messages.</p>}
      <p className="ob-returns-note">{returns.kind === "none" ? "No returns." : returns.kind === "days" ? `Returns within ${returns.days} days, ${returns.sellerPays ? "seller pays" : "buyer pays"} return shipping.` : "Returns follow the store's policy."}</p>
    </div>
  );
}
