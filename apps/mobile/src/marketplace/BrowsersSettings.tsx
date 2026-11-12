import { useState } from "react";
import { mintBrowserPairingCode } from "../catalog";
import { browsersCardCopy } from "../pairingUi";
import { CopyPaymentValue } from "./NodePayments";
import { FormError, useSubmit } from "./WalletLock";

/** Settings: mint a one-time `XXXX-XXXX` for another browser. */
export function BrowsersSettings() {
  const copy = browsersCardCopy();
  const [code, setCode] = useState("");
  const { busy, error, run } = useSubmit();
  return <div className="ob-settings-page">
    <header className="ob-settings-heading">
      <h1>{copy.pageTitle}</h1>
      <p>{copy.pageIntro}</p>
    </header>
    <section className="ob-settings-card" aria-labelledby="ob-browsers-card">
      <header className="ob-settings-card-heading">
        <h2 id="ob-browsers-card">{copy.cardTitle}</h2>
        <p>{copy.cardIntro}</p>
      </header>
      {code ? <div className="ob-browsers-code" role="status" aria-label="Pairing code">
        <CopyPaymentValue label="Pairing code" value={code}/>
        <p>{copy.shown}</p>
      </div> : null}
      <FormError message={error}/>
      <div className="ob-form-actions">
        <button type="button" className="ob-primary" disabled={busy} onClick={() => void run(async () => setCode(await mintBrowserPairingCode()), "A pairing code could not be made.")}>{busy ? "Creating…" : copy.action}</button>
      </div>
    </section>
  </div>;
}
