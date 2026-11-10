import { createContext, useEffect, useState, useSyncExternalStore, type ComponentType, type FormEvent } from "react";
import { onShopAwaitingCode, shopAwaitingCode } from "../catalog";
import { fetchHomeLink, shortAddress } from "../homeNode/api";
import { claimWithPasskey, createPasskey, passkeysSupported, signInWithPasskey } from "../homeNode/passkey";
import { parseAccessKey } from "../homeNode/signing";
import { nodeModerates } from "../moderator";
import { ModeratorDesk } from "./ModeratorDesk";
import { hasSession, nodeTarget, restoreSession, servedByHomeNode, signIn, signOut, switchTarget, webSignInState } from "../homeNode/transport";

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);
/** The page came over Tor from the node's onion, not from a web address (`--web-origin`). */
const overTor = () => typeof location !== "undefined" && location.hostname.endsWith(".onion");

/** Opens the moderator desk from the shop. Null when the node does not moderate or the shop is not paired yet. */
export const ModeratorDeskLink = createContext<(() => void) | null>(null);

/**
 * Wraps the shop. A page served by a home node first signs in this browser.
 * On a node that moderates, the shop and the moderator desk each link to the
 * other; a page served by such a node opens on the desk.
 */
export function withHomeNode(Shop: ComponentType) {
  return function HomeNodeRoot() {
    const [state, setState] = useState<"checking" | "signed-out" | "ready">(() => (!servedByHomeNode() || hasSession() ? "ready" : "checking"));
    // Whether the node moderates; null until it has said.
    const [moderates, setModerates] = useState<boolean | null>(null);
    // This computer's own shop opens at once. A served page waits to learn which to open.
    const [view, setView] = useState<"shop" | "desk" | null>(() => (servedByHomeNode() ? null : "shop"));
    useEffect(() => {
      if (state === "checking") void restoreSession().then(ok => setState(ok ? "ready" : "signed-out"));
      if (state !== "ready" || moderates !== null) return;
      // A node that could not be reached is asked again, so the desk is not lost until the page is reloaded.
      let retry: ReturnType<typeof setTimeout> | undefined;
      const ask = () => void nodeModerates().then(yes => {
        setModerates(yes);
        setView(now => now ?? (yes ? "desk" : "shop"));
      }, () => {
        setView(now => now ?? "shop");
        retry = setTimeout(ask, 15000);
      });
      ask();
      return () => clearTimeout(retry);
    }, [state, moderates]);
    // The desk's requests need the pairing the shop asks for, so a shop still waiting for its code does not offer the desk.
    const unpaired = useSyncExternalStore(onShopAwaitingCode, shopAwaitingCode);
    if (state === "ready" && view === "shop") return <ModeratorDeskLink.Provider value={moderates && !unpaired ? () => setView("desk") : null}><Shop/></ModeratorDeskLink.Provider>;
    if (state === "ready" && view === "desk") return <div className="ob-app"><StyleSheet/><ModeratorDesk onShop={() => setView("shop")}/></div>;
    return <div className="ob-app ob-home-gate">
      <StyleSheet/>
      {state === "signed-out" ? <SignIn onReady={() => setState("ready")}/> : <p role="status">{state === "checking" ? "Checking this browser’s key…" : "Asking the node…"}</p>}
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
  // On a web address: whether the node has a passkey. Null while that is asked, and on an onion.
  const [hasPasskey, setHasPasskey] = useState<boolean | null>(null);
  // A new node nobody owns yet: this browser may create its first passkey.
  const [claimable, setClaimable] = useState(false);
  // After a first sign-in with the setup key, offer to make the passkey that replaces it.
  const [offer, setOffer] = useState(false);
  const web = !overTor();
  const passkeys = web && passkeysSupported();
  const ask = () => webSignInState().then(node => { setHasPasskey(node.passkeys); setClaimable(node.claimable); }, () => setHasPasskey(false));
  useEffect(() => {
    if (web) void ask();
  }, [web]);

  async function run(work: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await work();
    } catch (err) {
      // Closing the device's passkey sheet is not an error worth showing.
      if (!(err instanceof DOMException && err.name === "NotAllowedError")) setError(failed(err, "The node could not be reached."));
    } finally {
      setBusy(false);
    }
  }
  const withKey = () => run(async () => {
    const secret = parseAccessKey(key);
    if (!secret) throw new Error(web ? "That is not a setup key. It starts with obkey-." : "That is not an access key. It starts with obkey-.");
    await signIn(secret, remember);
    if (passkeys && hasPasskey === false) setOffer(true);
    else onReady();
  });
  const withPasskey = () => run(async () => {
    await signInWithPasskey();
    onReady();
  });
  const makePasskey = () => run(async () => {
    await createPasskey();
    onReady();
  });
  const claim = () => run(async () => {
    try {
      await claimWithPasskey();
    } catch (err) {
      // Someone else may have just become the owner: show the page for the node as it is now.
      void ask();
      throw err;
    }
    onReady();
  });

  if (offer) return <div className="ob-home-gate-card">
    <img src="brand/ob-market-logo-256.png" alt="" width={48} height={48}/>
    <h1>Create a passkey</h1>
    <p>Next time, sign in with your fingerprint, face, or device PIN instead of the setup key. The passkey is kept by this device or your password manager.</p>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <button className="ob-primary" disabled={busy} onClick={() => void makePasskey()}>{busy ? "Waiting for your device…" : "Create a passkey"}</button>
    <button className="ob-secondary" disabled={busy} onClick={onReady}>Not now</button>
  </div>;

  const keyForm = <form className="ob-home-gate-form" onSubmit={(e: FormEvent) => { e.preventDefault(); void withKey(); }}>
    <label className="ob-field"><span>{web ? "Setup key" : "Access key"}</span>
      <input type="password" value={key} onChange={e => setKey(e.target.value)} placeholder="obkey-…" autoComplete="current-password" required autoFocus={!hasPasskey}/>
    </label>
    <label className="ob-check"><input type="checkbox" checked={remember} onChange={e => setRemember(e.target.checked)}/> Remember on this browser</label>
    <button className={hasPasskey ? "ob-secondary" : "ob-primary"} disabled={busy}>{busy ? "Checking…" : web && !hasPasskey ? "Continue" : "Sign in"}</button>
  </form>;

  if (web && hasPasskey === null) return <p role="status">Asking the node…</p>;
  if (claimable && passkeys) return <div className="ob-home-gate-card">
    <img src="brand/ob-market-logo-256.png" alt="" width={48} height={48}/>
    <h1>Set up this node</h1>
    <p>This node is new and has no owner yet. Create a passkey to make it yours. From then on you sign in here with your fingerprint, face, or device PIN.</p>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <button className="ob-primary" disabled={busy} onClick={() => void claim()}>{busy ? "Waiting for your device…" : "Create a passkey"}</button>
  </div>;
  return <div className="ob-home-gate-card">
    <img src="brand/ob-market-logo-256.png" alt="" width={48} height={48}/>
    {!web ? <>
      <h1>Your home node</h1>
      <p>This store runs on your own computer and is reached only over Tor. Sign in with this browser’s access key. To get one, open Settings, Home node on a paired device and choose Set up Tor Browser.</p>
    </> : hasPasskey ? <>
      <h1>Sign in to your node</h1>
      {passkeys
        ? <button className="ob-primary" disabled={busy} onClick={() => void withPasskey()}>{busy ? "Waiting for your device…" : "Sign in with a passkey"}</button>
        : <p>This browser cannot use passkeys here. Sign in with the setup key instead.</p>}
    </> : <>
      <h1>Set up this node</h1>
      <p>Paste the setup key from the service that hosts this node. On Railway, open the service, then Variables, and copy OB_WEB_ACCESS_KEY.{passkeys ? " You need it only this once: next you create a passkey." : ""}</p>
    </>}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    {web && hasPasskey && passkeys ? <details>
      <summary>Trouble signing in?</summary>
      <p>If you never created a passkey here, someone else set this node up first: delete its data on the hosting service and deploy it again. If you lost your passkey, set OB_WEB_ACCESS_KEY on the hosting service, restart the node, and sign in with that key.</p>
      {keyForm}
    </details> : keyForm}
  </div>;
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
    {served ? (overTor() ? "Managing your home node over Tor" : "Managing your home node") : `Managing your home node${address ? ` ${shortAddress(address)}` : ""}`}
    {served
      ? <button className="ob-text-link" onClick={() => { signOut(); location.reload(); }}>Sign out</button>
      : <button className="ob-text-link" onClick={() => switchTarget("local")}>Use this computer's node</button>}
  </div>;
}
