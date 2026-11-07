import { useState } from "react";
import { setRole, type Role, type SellerProfile } from "../seller";
import { usePairedNode } from "./NodePayments";
import { WalletLockPanel } from "./WalletLock";

/**
 * First run: protect the wallet, then choose to shop or to sell. Shown until
 * the node has a role; a buyer can open a store later from Settings.
 */
export function Onboarding({ onChosen }: { onChosen: (profile: SellerProfile) => void }) {
  const { lock } = usePairedNode();
  const [busy, setBusy] = useState<Role | null>(null);
  const [error, setError] = useState("");
  const setup = lock.data?.state === "setup";
  async function choose(role: Role) {
    if (busy) return;
    setBusy(role);
    setError("");
    try {
      onChosen(await setRole(role));
    } catch (e) {
      setError(e instanceof Error ? e.message : "Your node could not save that choice.");
      setBusy(null);
    }
  }
  return <main className="ob-onboarding" id="ob-content">
    <div className="ob-onboarding-inner">
      <header className="ob-onboarding-brand"><img src="brand/ob-market-logo-256.png" alt="" width={48} height={48}/><span>OpenBazaar</span></header>
      <p className="ob-onboarding-step">Step {setup || !lock.data ? 1 : 2} of 2</p>
      {setup || !lock.data ? <WalletLockPanel lock={lock}/> : <section className="ob-role-choice" aria-labelledby="ob-role-title">
        <h1 id="ob-role-title">How will you use OpenBazaar?</h1>
        <p>Your node keeps your wallet either way. You can open a store later from Settings.</p>
        <div className="ob-role-options">
          <button className="ob-role-option" disabled={!!busy} onClick={() => void choose("buyer")}>
            <span className="ob-role-art is-shop" aria-hidden="true"/>
            <strong>Shop</strong>
            <span>Browse stores and pay sellers directly with bitcoin.</span>
            <em>{busy === "buyer" ? "Saving…" : "Start shopping"}</em>
          </button>
          <button className="ob-role-option is-sell" disabled={!!busy} onClick={() => void choose("seller")}>
            <span className="ob-role-art is-sell" aria-hidden="true"/>
            <strong>Sell</strong>
            <span>Open a store, list goods, and take orders on your own node.</span>
            <em>{busy === "seller" ? "Saving…" : "Set up my store"}</em>
          </button>
        </div>
        {error && <p className="ob-form-error" role="alert">{error}</p>}
      </section>}
    </div>
  </main>;
}
