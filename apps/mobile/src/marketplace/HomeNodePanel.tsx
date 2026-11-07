import { useCallback, useEffect, useState, type FormEvent } from "react";
import QRCode from "qrcode";
import {
  createPairingCode,
  fetchDevices,
  fetchHomeLink,
  fetchRemoteStatus,
  forgetHome,
  pairHome,
  revokeDevice,
  shortAddress,
  type HomeLink,
  type RemoteDevice,
  type RemoteStatus,
} from "../homeNode/api";
import { nodeTarget, servedByHomeNode, signOut, switchTarget } from "../homeNode/transport";
import { CopyPaymentValue } from "./NodePayments";

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);
const when = (secs: number) => (secs ? new Date(secs * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "never");

/** Settings > Home node: connect this app to a home node, and manage the devices of the node in use. */
export function HomeNodePanel() {
  const served = servedByHomeNode();
  return <section className="ob-home-node" aria-label="Home node">
    {!served && <ConnectionCard/>}
    <DevicesCard/>
  </section>;
}

function ConnectionCard() {
  const [link, setLink] = useState<HomeLink | null>(null);
  const [error, setError] = useState("");
  const [code, setCode] = useState("");
  const [name, setName] = useState("This computer");
  const [busy, setBusy] = useState(false);
  const target = nodeTarget();
  useEffect(() => { void fetchHomeLink().then(setLink, e => setError(failed(e, "This app's node could not be reached."))); }, []);

  async function connect(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      setLink(await pairHome(code, name));
      setCode("");
    } catch (err) {
      setError(failed(err, "The home node could not be paired."));
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
      <p>Manage a store that runs on an always-on computer at home. This app's node reaches it over Tor with a key made for this device. There is no account and no server in between.</p>
    </div>
    {link?.paired ? <div className="ob-home-link">
      <div className="ob-home-link-row">
        <span className={`ob-status-dot ${target === "home" ? "" : "is-loading"}`} aria-hidden="true"/>
        <div>
          <strong>{target === "home" ? "Managing your home node" : "Home node paired"}</strong>
          <span><code title={link.address}>{shortAddress(link.address)}</code> · this device is “{link.name}” · paired {when(link.pairedAt)}</span>
        </div>
      </div>
      <div className="ob-form-actions">
        {target === "home"
          ? <button className="ob-secondary" onClick={() => switchTarget("local")}>Use this computer's node</button>
          : <button className="ob-primary" onClick={() => switchTarget("home")}>Manage home node</button>}
        <button className="ob-secondary" disabled={busy} onClick={() => void forget()}>Forget home node</button>
      </div>
    </div> : <form className="ob-home-connect" onSubmit={e => void connect(e)}>
      <label className="ob-field ob-field-wide"><span>Pairing code</span>
        <input value={code} onChange={e => setCode(e.target.value)} placeholder="obhome:…@….onion" autoComplete="off" autoCapitalize="none" spellCheck={false} required/>
        <small>Run <code>ob-node pair</code> on the home node, or copy the code from its log on first start, or from Devices on a paired device. It works once, for 15 minutes.</small>
      </label>
      <label className="ob-field"><span>Name this device</span>
        <input value={name} onChange={e => setName(e.target.value)} maxLength={40} required/>
      </label>
      <div className="ob-form-actions">
        <button className="ob-primary" disabled={busy || !code.trim().startsWith("obhome:")}>{busy ? "Connecting over Tor…" : "Connect"}</button>
      </div>
      {busy && <p className="ob-settings-footnote" role="status">The first connection builds a Tor circuit and can take up to a minute.</p>}
    </form>}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
  </div>;
}

function DevicesCard() {
  const served = servedByHomeNode();
  const remoteTarget = served || nodeTarget() === "home";
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [devices, setDevices] = useState<RemoteDevice[] | null>(null);
  const [offer, setOffer] = useState<{ code: string; expires: number } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextDevices] = await Promise.all([fetchRemoteStatus(), fetchDevices()]);
      setStatus(nextStatus);
      setDevices(nextDevices);
    } catch (err) {
      setError(failed(err, "The node's devices could not be read."));
    }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);

  async function pairAnother() {
    setBusy(true);
    setError("");
    try {
      const next = await createPairingCode();
      setOffer({ code: next.code, expires: Date.now() + next.expiresInSecs * 1000 });
    } catch (err) {
      setError(failed(err, "A pairing code could not be made."));
    } finally {
      setBusy(false);
    }
  }

  async function remove(device: RemoteDevice) {
    const others = devices?.filter(d => d.pairedBy === device.id).length ?? 0;
    const lead = device.thisDevice ? "Remove this device? It signs out now." : `Remove “${device.name}”? It can no longer reach this node.`;
    if (!window.confirm(others ? `${lead} Devices it paired are removed too.` : lead)) return;
    setBusy(true);
    try {
      setDevices(await revokeDevice(device.id));
      if (device.thisDevice && served) {
        signOut();
        location.reload();
      }
    } catch (err) {
      setError(failed(err, "The device could not be removed."));
    } finally {
      setBusy(false);
    }
  }

  const nameOf = (id: string | null) => (id ? devices?.find(d => d.id === id)?.name ?? "a removed device" : "the node itself");
  return <div className="ob-settings-card">
    <div className="ob-settings-card-heading">
      <h2>{remoteTarget ? "Devices paired with your home node" : "Devices paired with this node"}</h2>
      <p>Each device holds its own key. Remove a lost or stolen device here; anything it paired is removed with it. Paying from the wallet still needs the wallet passphrase on every device.</p>
    </div>
    {status && !status.enabled && <p className="ob-settings-notice">Remote access is off on this node. Start it with <code>ob-node serve --remote</code> to pair devices.</p>}
    {status?.onion && <CopyPaymentValue label="Onion address" value={`http://${status.onion}/`}/>}
    {devices === null ? null : devices.length === 0
      ? <div className="ob-settings-empty">No device is paired yet.</div>
      : <ul className="ob-labeler-list ob-device-list">{devices.map(device => <li key={device.id}>
          <div>
            <strong>{device.name}{device.thisDevice && <span className="ob-device-tag">This device</span>}</strong>
            <span>Paired {when(device.pairedAt)} by {nameOf(device.pairedBy)} · last seen {when(device.lastSeen)}</span>
            <code>{device.id}</code>
          </div>
          <button className="ob-secondary" disabled={busy} onClick={() => void remove(device)}>Remove</button>
        </li>)}</ul>}
    {status?.enabled && <div className="ob-form-actions ob-device-actions">
      <button className="ob-primary" disabled={busy} onClick={() => void pairAnother()}>Pair a new device</button>
      <button className="ob-secondary" disabled={busy} onClick={() => void refresh()}>Refresh</button>
    </div>}
    {offer && <PairingOffer code={offer.code} expires={offer.expires} onDone={() => { setOffer(null); void refresh(); }}/>}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
  </div>;
}

function PairingOffer({ code, expires, onDone }: { code: string; expires: number; onDone: () => void }) {
  const [qr, setQr] = useState("");
  const [left, setLeft] = useState(() => Math.max(0, Math.round((expires - Date.now()) / 1000)));
  useEffect(() => {
    let active = true;
    void QRCode.toDataURL(code, { width: 208, margin: 2, errorCorrectionLevel: "M", color: { dark: "#063853", light: "#ffffff" } }).then(src => { if (active) setQr(src); }, () => undefined);
    return () => { active = false; };
  }, [code]);
  useEffect(() => {
    const timer = setInterval(() => setLeft(Math.max(0, Math.round((expires - Date.now()) / 1000))), 1000);
    return () => clearInterval(timer);
  }, [expires]);
  return <div className="ob-pairing-offer" role="group" aria-label="New pairing code">
    {qr && <img src={qr} width={208} height={208} alt="Pairing code as a QR code"/>}
    <div>
      <h3>{left > 0 ? `Pair within ${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` : "This code has expired"}</h3>
      <p>In the OpenBazaar app on the other device: Settings, Home node, Connect, and paste this code. In Tor Browser: open the onion address and paste it. It pairs one device, once.</p>
      <CopyPaymentValue label="Pairing code" value={code}/>
      <button className="ob-text-link" onClick={onDone}>Done</button>
    </div>
  </div>;
}
