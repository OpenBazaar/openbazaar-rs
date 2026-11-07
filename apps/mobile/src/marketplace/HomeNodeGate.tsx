import { useEffect, useState, type ComponentType, type FormEvent } from "react";
import { fetchHomeLink, shortAddress } from "../homeNode/api";
import { parseAccessKey } from "../homeNode/signing";
import { hasSession, nodeTarget, pairBrowser, restoreSession, servedByHomeNode, signIn, signOut, switchTarget } from "../homeNode/transport";
import { CopyPaymentValue } from "./NodePayments";

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

/** Wraps the shop. A page served by a home node first pairs or signs in this browser. */
export function withHomeNode(Shop: ComponentType) {
  return function HomeNodeRoot() {
    const [state, setState] = useState<"checking" | "signed-out" | "ready">(() => (!servedByHomeNode() || hasSession() ? "ready" : "checking"));
    useEffect(() => {
      if (state === "checking") void restoreSession().then(ok => setState(ok ? "ready" : "signed-out"));
    }, [state]);
    if (state === "ready") return <Shop/>;
    return <div className="ob-app ob-home-gate">
      <StyleSheet/>
      {state === "checking" ? <p role="status">Checking this browser’s key…</p> : <SignIn onReady={() => setState("ready")}/>}
    </div>;
  };
}

function StyleSheet() {
  useEffect(() => {
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = "marketplace.css";
    document.head.appendChild(link);
    document.title = "OpenBazaar home node";
    return () => link.remove();
  }, []);
  return null;
}

function SignIn({ onReady }: { onReady: () => void }) {
  const [mode, setMode] = useState<"pair" | "key">("pair");
  const [code, setCode] = useState("");
  const [name, setName] = useState("Tor Browser");
  const [key, setKey] = useState("");
  const [remember, setRemember] = useState(false);
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      if (mode === "pair") {
        setSaved(await pairBrowser(code, name, remember));
      } else {
        const secret = parseAccessKey(key);
        if (!secret) throw new Error("That is not an access key. It starts with obkey-.");
        await signIn(secret, remember);
        onReady();
      }
    } catch (err) {
      setError(failed(err, "The node could not be reached."));
    } finally {
      setBusy(false);
    }
  }

  if (saved) return <section className="ob-home-gate-card" aria-label="Save your access key">
    <img src="brand/ob-market-logo-256.png" alt="" width={48} height={48}/>
    <h1>Save this browser’s access key</h1>
    <p>This browser is paired. Tor Browser forgets site data when it closes, so keep this key in your password manager and paste it to sign in next time. Anyone with it can manage your store until you remove this device.</p>
    <CopyPaymentValue label="Access key" value={saved}/>
    <button className="ob-primary" onClick={onReady}>I saved it — open my store</button>
  </section>;

  return <form className="ob-home-gate-card" onSubmit={e => void submit(e)}>
    <img src="brand/ob-market-logo-256.png" alt="" width={48} height={48}/>
    <h1>Your home node</h1>
    <p>This store runs on your own computer and is reached only over Tor. Pair this browser with a one-time code from the node, or sign in with the access key you saved.</p>
    <div className="ob-home-gate-tabs" role="tablist">
      <button type="button" role="tab" aria-selected={mode === "pair"} className={mode === "pair" ? "is-active" : ""} onClick={() => setMode("pair")}>Pair this browser</button>
      <button type="button" role="tab" aria-selected={mode === "key"} className={mode === "key" ? "is-active" : ""} onClick={() => setMode("key")}>Use an access key</button>
    </div>
    {mode === "pair" ? <>
      <label className="ob-field"><span>Pairing code</span>
        <input value={code} onChange={e => setCode(e.target.value)} placeholder="obhome:…@….onion" autoComplete="off" autoCapitalize="none" spellCheck={false} required autoFocus/>
        <small>Run <code>ob-node pair</code> on the node, or make one under Settings, Home node on a paired device.</small>
      </label>
      <label className="ob-field"><span>Name this browser</span><input value={name} onChange={e => setName(e.target.value)} maxLength={40} required/></label>
    </> : <label className="ob-field"><span>Access key</span>
      <input type="password" value={key} onChange={e => setKey(e.target.value)} placeholder="obkey-…" autoComplete="current-password" required autoFocus/>
    </label>}
    <label className="ob-check"><input type="checkbox" checked={remember} onChange={e => setRemember(e.target.checked)}/> Remember on this browser</label>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <button className="ob-primary" disabled={busy}>{busy ? "Checking…" : mode === "pair" ? "Pair this browser" : "Sign in"}</button>
  </form>;
}

/** A banner while the shop manages a node other than this app's own. */
export function HomeNodeBadge() {
  const served = servedByHomeNode();
  const [address, setAddress] = useState("");
  const home = !served && nodeTarget() === "home";
  useEffect(() => {
    if (home) void fetchHomeLink().then(link => setAddress(link.paired ? link.address : ""), () => undefined);
  }, [home]);
  if (!served && !home) return null;
  return <div className="ob-network-badge is-home" role="status">
    <span className="ob-status-dot" aria-hidden="true"/>
    {served ? "Managing your home node over Tor" : `Managing your home node${address ? ` ${shortAddress(address)}` : ""} over Tor`}
    {served
      ? <button className="ob-text-link" onClick={() => { signOut(); location.reload(); }}>Sign out</button>
      : <button className="ob-text-link" onClick={() => switchTarget("local")}>Use this computer's node</button>}
  </div>;
}
