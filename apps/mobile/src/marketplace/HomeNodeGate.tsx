import { useEffect, useState, type ComponentType, type FormEvent } from "react";
import { fetchHomeLink, shortAddress } from "../homeNode/api";
import { parseAccessKey } from "../homeNode/signing";
import { hasSession, nodeTarget, restoreSession, servedByHomeNode, signIn, signOut, switchTarget } from "../homeNode/transport";

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
  const [key, setKey] = useState("");
  const [remember, setRemember] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const secret = parseAccessKey(key);
      if (!secret) throw new Error("That is not an access key. It starts with obkey-.");
      await signIn(secret, remember);
      onReady();
    } catch (err) {
      setError(failed(err, "The node could not be reached."));
    } finally {
      setBusy(false);
    }
  }

  return <form className="ob-home-gate-card" onSubmit={e => void submit(e)}>
    <img src="brand/ob-market-logo-256.png" alt="" width={48} height={48}/>
    <h1>Your home node</h1>
    <p>This store runs on your own computer and is reached only over Tor. Sign in with this browser’s access key. To get one, open Settings, Home node on a paired device and choose Set up Tor Browser.</p>
    <label className="ob-field"><span>Access key</span>
      <input type="password" value={key} onChange={e => setKey(e.target.value)} placeholder="obkey-…" autoComplete="current-password" required autoFocus/>
    </label>
    <label className="ob-check"><input type="checkbox" checked={remember} onChange={e => setRemember(e.target.checked)}/> Remember on this browser</label>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <button className="ob-primary" disabled={busy}>{busy ? "Checking…" : "Sign in"}</button>
  </form>;
}

/** A banner while the shop manages a node other than this app's own. */
export function HomeNodeBadge() {
  const served = servedByHomeNode();
  const [address, setAddress] = useState("");
  const home = !served && nodeTarget() === "home";
  useEffect(() => {
    if (home) void fetchHomeLink().then(link => setAddress(link.paired ? link.onion : ""), () => undefined);
  }, [home]);
  if (!served && !home) return null;
  return <div className="ob-network-badge is-home" role="status">
    <span className="ob-status-dot" aria-hidden="true"/>
    {served ? "Managing your home node over Tor" : `Managing your home node${address ? ` ${shortAddress(address)}` : ""}`}
    {served
      ? <button className="ob-text-link" onClick={() => { signOut(); location.reload(); }}>Sign out</button>
      : <button className="ob-text-link" onClick={() => switchTarget("local")}>Use this computer's node</button>}
  </div>;
}
