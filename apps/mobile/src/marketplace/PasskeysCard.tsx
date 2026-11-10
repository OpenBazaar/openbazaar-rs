import { useEffect, useState } from "react";
import { createPasskey, fetchPasskeys, passkeysSupported, removePasskey, type Passkey, type PasskeyInfo } from "../homeNode/passkey";

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);
const when = (secs: number) => (secs ? new Date(secs * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "an unknown date");

/** The passkeys that sign a browser in on the node's web address. Renders nothing on an onion or a node without a web address. */
export function PasskeysCard() {
  const [info, setInfo] = useState<PasskeyInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const supported = passkeysSupported();
  useEffect(() => {
    if (supported) void fetchPasskeys().then(setInfo, () => setInfo(null));
  }, [supported]);
  if (!supported || !info?.enabled) return null;

  async function act(work: () => Promise<PasskeyInfo>, fallback: string) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      setInfo(await work());
    } catch (err) {
      // Closing the device's passkey sheet is not an error worth showing.
      if (!(err instanceof DOMException && err.name === "NotAllowedError")) setError(failed(err, fallback));
    } finally {
      setBusy(false);
    }
  }
  async function remove(passkey: Passkey) {
    if (!window.confirm("Remove this passkey? Every browser it signed in is signed out, and devices paired from those browsers are removed.")) return;
    await act(() => removePasskey(passkey.id), "The passkey could not be removed.");
    // This browser may have been one of them.
    void fetchPasskeys().catch(() => location.reload());
  }

  return <div className="ob-settings-card">
    <div className="ob-settings-card-heading">
      <h2>Passkeys</h2>
      <p>Sign in to this page with your fingerprint, face, or device PIN instead of the setup key. A passkey signs you in; the wallet still asks for its passphrase.</p>
    </div>
    {info.passkeys.length === 0
      ? <div className="ob-settings-empty">No passkey yet. Until you create one, signing in needs the setup key.</div>
      : <ul className="ob-labeler-list ob-device-list">{info.passkeys.map(passkey => <li key={passkey.id}>
          <div>
            <strong>{passkey.name}</strong>
            <span>Created {when(passkey.addedAt)}</span>
          </div>
          <button className="ob-secondary" disabled={busy} onClick={() => void remove(passkey)}>Remove</button>
        </li>)}</ul>}
    <div className="ob-form-actions ob-device-actions">
      <button className="ob-primary" disabled={busy} onClick={() => void act(createPasskey, "The passkey could not be created.")}>{busy ? "Waiting for your device…" : "Create a passkey"}</button>
    </div>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
  </div>;
}
