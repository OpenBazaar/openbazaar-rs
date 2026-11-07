import { useEffect, useState, type FormEvent } from "react";
import { fetchHomeLink, forgetHome, shortAddress, startHomePairing, waitHomePairing, type HomeLink, type HomePairing } from "../homeNode/api";
import { nodeTarget, switchTarget } from "../homeNode/transport";
import { Sas } from "./HomePairingParts";

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);
const when = (secs: number) => (secs ? new Date(secs * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "never");

/** Connect this app to a home node: words or link, compare the emoji, approve on the node. */
export function HomeConnectCard() {
  const [link, setLink] = useState<HomeLink | null>(null);
  const [waiting, setWaiting] = useState<HomePairing | null>(null);
  const [error, setError] = useState("");
  const [code, setCode] = useState("");
  const [name, setName] = useState("This computer");
  const [busy, setBusy] = useState(false);
  const target = nodeTarget();
  useEffect(() => { void fetchHomeLink().then(setLink, e => setError(failed(e, "This app's node could not be reached."))); }, []);

  useEffect(() => {
    if (!waiting) return;
    let active = true;
    void (async () => {
      while (active) {
        try {
          const answer = await waitHomePairing(waiting.requestId);
          if (!active) return;
          if (answer.state === "pending") continue;
          if (answer.state === "approved") {
            setLink(answer.link);
            setCode("");
          } else {
            setError(answer.state === "rejected" ? "The home node turned this device down." : "The pairing code expired. Make a new one on the home node.");
          }
        } catch (err) {
          if (active) setError(failed(err, "The home node stopped answering."));
        }
        if (active) setWaiting(null);
        return;
      }
    })();
    return () => { active = false; };
  }, [waiting]);

  async function connect(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      setWaiting(await startHomePairing(code, name));
    } catch (err) {
      setError(failed(err, "The home node could not be reached."));
    } finally {
      setBusy(false);
    }
  }

  async function forget() {
    if (busy || !window.confirm("Forget the home node on this computer? The home node is asked to remove this device.")) return;
    setBusy(true);
    try {
      setLink(await forgetHome());
      if (target === "home") switchTarget("local");
    } catch (err) {
      setError(failed(err, "The home node could not be forgotten."));
    } finally {
      setBusy(false);
    }
  }

  return <div className="ob-settings-card">
    <div className="ob-settings-card-heading">
      <h2>Connect to my home node</h2>
      <p>Manage a store that runs on an always-on computer at home. This app's node reaches it on your home network or over Tor, with a key made for this device. There is no account and no server in between.</p>
    </div>
    {link?.paired ? <div className="ob-home-link">
      <div className="ob-home-link-row">
        <span className={`ob-status-dot ${target === "home" ? "" : "is-loading"}`} aria-hidden="true"/>
        <div>
          <strong>{target === "home" ? "Managing your home node" : "Home node paired"}</strong>
          <span>{link.onion && <><code title={link.onion}>{shortAddress(link.onion)}</code> · </>}this device is “{link.name}” · paired {when(link.pairedAt)}</span>
        </div>
      </div>
      <div className="ob-form-actions">
        {target === "home"
          ? <button className="ob-secondary" onClick={() => switchTarget("local")}>Use this computer's node</button>
          : <button className="ob-primary" onClick={() => switchTarget("home")}>Manage home node</button>}
        <button className="ob-secondary" disabled={busy} onClick={() => void forget()}>Forget home node</button>
      </div>
    </div> : waiting ? <div className="ob-home-connect" role="status">
      <Sas sas={waiting.sas} names={waiting.sasNames}/>
      <p>Check that your home node shows the same four emoji, then approve this device there: on its setup page, in its terminal, or on a device that is already paired. Do not approve if they differ.</p>
      <p className="ob-settings-footnote">Waiting for the home node…</p>
      <div className="ob-form-actions"><button className="ob-secondary" onClick={() => setWaiting(null)}>Cancel</button></div>
    </div> : <form className="ob-home-connect" onSubmit={e => void connect(e)}>
      <label className="ob-field ob-field-wide"><span>Pairing code</span>
        <input value={code} onChange={e => setCode(e.target.value)} placeholder="eight words, or openbazaar://pair?…" autoComplete="off" autoCapitalize="none" spellCheck={false} required/>
        <small>The home node shows it on its setup page (openbazaar.local:8790 on your network), in its log on first start, with <code>ob-node pair</code>, or under Devices on a paired device. It works once, for 10 minutes.</small>
      </label>
      <label className="ob-field"><span>Name this device</span>
        <input value={name} onChange={e => setName(e.target.value)} maxLength={40} required/>
      </label>
      <div className="ob-form-actions">
        <button className="ob-primary" disabled={busy || !code.trim()}>{busy ? "Connecting privately…" : "Connect"}</button>
      </div>
      {busy && <p className="ob-settings-footnote" role="status">Away from home the first connection builds Tor circuits and can take up to a minute.</p>}
    </form>}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
  </div>;
}
