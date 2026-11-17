import { useState } from "react";
import { fetchLnStores, setStoreAcceptsLightning, type LnStores } from "../lightning";
import { lightningOffer, moneroOffer, zcashOffer, type DirectRail, type LightningRail, type RailOffer } from "../paymentRails";
import { fetchProfile, type SellerProfile } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { fetchXmrStores, setStoreAcceptsXmr } from "../xmr";
import { fetchZecStores, setStoreAcceptsZec } from "../zec";
import { EscrowMark, RailMark, type Rail } from "./RailMark";
import { Dialog, FormError, useSubmit } from "./WalletLock";

type Choice = "monero" | "lightning" | "zcash";

/** The own store's row, or the row under the saved name. */
function ownRow<T extends { seller: string; own: boolean }>(stores: T[] | undefined, name: string): T | undefined {
  return stores?.find((store) => store.own) ?? stores?.find((store) => store.seller === name);
}

function directFrom(data: { enabled: boolean } | null, error: string, accepts: boolean, problem: string | null): DirectRail | null {
  if (!data) return error ? { enabled: false, accepts, problem: error } : null;
  return { enabled: data.enabled, accepts, problem };
}

/**
 * What this store accepts. Bitcoin is always on. Monero, Lightning, and Zcash
 * are switches that sign the store record again. A rail the node cannot take
 * stays visible, with the node's reason, instead of disappearing.
 */
export function StorePayments({ profile, onChanged }: { profile: SellerProfile; onChanged: (profile: SellerProfile, message: string) => void }) {
  const xmr = useNodeResource(fetchXmrStores, 0);
  const zec = useNodeResource(fetchZecStores, 0);
  const ln = useNodeResource(fetchLnStores, 0);
  const toggle = useSubmit();
  const [ack, setAck] = useState(false);
  const xmrOwn = ownRow(xmr.data?.stores, profile.name);
  const zecOwn = ownRow(zec.data?.stores, profile.name);
  const lnOwn = ownRow(ln.data?.stores, profile.name);
  const seller = xmrOwn?.seller ?? zecOwn?.seller ?? lnOwn?.seller ?? profile.name;
  const monero = moneroOffer(profile.published, directFrom(xmr.data, xmr.error, xmrOwn?.acceptsXmr ?? profile.acceptsXmr, xmrOwn?.problem ?? null));
  const zcash = zcashOffer(profile.published, directFrom(zec.data, zec.error, zecOwn?.acceptsZec ?? false, zecOwn?.problem ?? null));
  const lightning = lightningOffer(profile.published, lnFrom(ln.data, ln.error, lnOwn?.acceptsLightning ?? profile.acceptsLightning));
  const testnet = profile.network === "testnet4";

  async function commit(rail: Choice, on: boolean, acknowledgeLsp = false) {
    const label = rail === "monero" ? "Monero" : rail === "lightning" ? "Lightning" : "Zcash";
    await toggle.run(async () => {
      if (rail === "monero") xmr.update(await setStoreAcceptsXmr(seller, on));
      else if (rail === "zcash") zec.update(await setStoreAcceptsZec(seller, on));
      else ln.update(await setStoreAcceptsLightning(seller, on, acknowledgeLsp));
      setAck(false);
      onChanged(await fetchProfile(), on ? `Your store takes ${label}.` : `Your store no longer takes ${label}.`);
    }, "The store could not be signed again.");
  }

  function choose(rail: Choice, on: boolean) {
    if (rail === "lightning" && on && lightning.needsLspAck) {
      setAck(true);
      return;
    }
    void commit(rail, on);
  }

  return (
    <fieldset className="ob-store-section">
      <legend>Payments</legend>
      <ul className="ob-payment-rails">
        <li className="is-on">
          <strong className="ob-rail"><RailMark rail="bitcoin" />Bitcoin</strong>
          <p>{testnet ? "Testnet4 wallet. " : ""}Each accepted order receives a fresh payment address. New sellers must satisfy the node's escrow and reputation requirements before accepting orders.</p>
        </li>
        <ChoiceRow rail="monero" title="Monero" offer={monero} busy={toggle.busy} onToggle={(on) => choose("monero", on)} detail={`${testnet ? "Stagenet test coins. " : ""}Buyers pay a subaddress that your node watches. Direct pay only, no escrow. Experimental.`} />
        <ChoiceRow rail="lightning" title="Lightning" offer={lightning} busy={toggle.busy} onToggle={(on) => choose("lightning", on)} detail={`${testnet ? "Mutinynet signet test coins, a test network separate from your testnet4 bitcoin. " : ""}Buyers pay a Lightning invoice. It stays off on mainnet until connections run over Tor. Direct pay only, no escrow. Experimental.`} />
        <ChoiceRow rail="zcash" title="Zcash" offer={zcash} busy={toggle.busy} onToggle={(on) => choose("zcash", on)} detail={`${testnet ? "Zcash testnet coins. " : ""}Buyers pay a shielded address that your node watches. Your own store takes Zcash on testnet4 only for now. Direct pay only, no escrow.`} />
        <li className={profile.escrowAvailable ? "" : "is-off"}>
          <strong className="ob-rail"><EscrowMark />Moderated escrow</strong>
          <p>{profile.escrowAvailable ? "Buyers can choose a moderator. Your listings take escrow on testnet4. Escrow pays in bitcoin." : "Moderated escrow runs on the test network only. Escrow pays in bitcoin."}</p>
        </li>
      </ul>
      <FormError message={toggle.error} />
      {ack && ln.data?.lspNotice && (
        <Dialog label="How the LSP sees your payments" onCancel={() => setAck(false)}>
          <div className="ob-spend-confirm">
            <h2>How the LSP sees your payments</h2>
            <p className="ob-page-intro">{ln.data.lspNotice}</p>
            <FormError message={toggle.error} />
            <div className="ob-form-actions">
              <button className="ob-secondary" type="button" onClick={() => setAck(false)}>Cancel</button>
              <button className="ob-primary" disabled={toggle.busy} onClick={() => void commit("lightning", true, true)}>{toggle.busy ? "Signing…" : "I understand, take Lightning"}</button>
            </div>
          </div>
        </Dialog>
      )}
    </fieldset>
  );
}

function lnFrom(data: LnStores | null, error: string, accepts: boolean): LightningRail | null {
  if (!data) {
    return error ? { enabled: false, blocked: null, accepts, problem: error, lspNotice: null, lspAcknowledged: false } : null;
  }
  return { enabled: data.enabled, blocked: data.blocked, accepts, problem: null, lspNotice: data.lspNotice, lspAcknowledged: data.lspAcknowledged };
}

function ChoiceRow({ rail, title, detail, offer, busy, onToggle }: { rail: Rail; title: string; detail: string; offer: RailOffer; busy: boolean; onToggle: (on: boolean) => void }) {
  const id = `ob-store-rail-${rail}`;
  return (
    <li className={offer.on ? "is-on" : offer.choosable ? "" : "is-off"}>
      <div className="ob-payment-rail-head">
        <strong className="ob-rail" id={id}><RailMark rail={rail} />{title}</strong>
        <span className="ob-setting-switch">
          <input type="checkbox" role="switch" aria-labelledby={id} checked={offer.on} disabled={!offer.choosable || busy} onChange={(e) => onToggle(e.target.checked)} />
          <span />
        </span>
      </div>
      <p>{detail}</p>
      {offer.reason && <p className="ob-form-error" role="alert">{offer.reason}</p>}
      {offer.problem && <p className="ob-form-error" role="status">{offer.problem}</p>}
    </li>
  );
}
