import { useCallback, useEffect, useState } from "react";
import {
  createBrowserCredential,
  createPairingCode,
  decidePending,
  fetchDevices,
  fetchPending,
  fetchRemoteStatus,
  revokeDevice,
  type BrowserCredential,
  type PairingCode,
  type PendingDevice,
  type RemoteDevice,
  type RemoteStatus,
} from "../homeNode/api";
import { nodeTarget, servedByHomeNode, signOut } from "../homeNode/transport";
import { BrowserCredentialOffer, PairingOffer, Sas } from "./HomePairingParts";
import { CopyPaymentValue } from "./NodePayments";
import { Dialog } from "./WalletLock";

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);
const when = (secs: number) => (secs ? new Date(secs * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "never");
const PLATFORMS: Record<string, string> = { ios: "iPhone", android: "Android", desktop: "Desktop", browser: "Browser" };
const VIA: Record<string, string> = { tor: "over Tor", lan: "on your network", local: "on the node", web: "on the web" };

/** The devices of the node in use: waiting approvals, the list, pairing, removal. */
export function HomeDevicesCard({ presentation = "default" }: { presentation?: "default" | "desk" }) {
  const served = servedByHomeNode();
  const remoteTarget = served || nodeTarget() === "home";
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [devices, setDevices] = useState<RemoteDevice[] | null>(null);
  const [pending, setPending] = useState<PendingDevice[]>([]);
  const [offer, setOffer] = useState<{ code: PairingCode; expires: number } | null>(null);
  const [credential, setCredential] = useState<BrowserCredential | null>(null);
  // The name being typed for a new Tor Browser, or null while that form is closed.
  const [naming, setNaming] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextDevices, nextPending] = await Promise.all([fetchRemoteStatus(), fetchDevices(), fetchPending()]);
      setStatus(nextStatus);
      setDevices(nextDevices);
      setPending(nextPending);
      setError("");
    } catch (err) {
      setError(failed(err, "The node's devices could not be read."));
    }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    if (!offer) return;
    const timer = setInterval(() => void fetchPending().then(setPending, () => undefined), 4000);
    return () => clearInterval(timer);
  }, [offer]);

  async function act(work: () => Promise<void>, fallback: string) {
    setBusy(true);
    setError("");
    try {
      await work();
    } catch (err) {
      setError(failed(err, fallback));
    } finally {
      setBusy(false);
    }
  }

  const pairAnother = () => act(async () => {
    const code = await createPairingCode();
    setCredential(null);
    setOffer({ code, expires: Date.now() + code.expiresInSecs * 1000 });
  }, "A pairing code could not be made.");

  const browser = (name: string) => act(async () => {
    setOffer(null);
    setCredential(await createBrowserCredential(name));
    setNaming(null);
    await refresh();
  }, "A browser credential could not be made.");

  const decide = (request: PendingDevice, approve: boolean) => act(async () => {
    await decidePending(request.requestId, approve);
    if (approve) setOffer(null);
    await refresh();
  }, "The answer could not be sent.");

  const remove = (device: RemoteDevice) => act(async () => {
    const others = devices?.filter(d => d.pairedBy === device.id).length ?? 0;
    const lead = device.thisDevice ? "Remove this device? It signs out now." : `Remove “${device.name}”? It can no longer reach this node.`;
    if (!window.confirm(others ? `${lead} Devices it paired are removed too.` : lead)) return;
    const rotate = !device.thisDevice && window.confirm("Also change the node's onion address? Do this if the device was lost or stolen. Your other devices move to the new address by themselves.");
    setDevices(await revokeDevice(device.id, rotate));
    if (device.thisDevice && served) {
      signOut();
      location.reload();
    }
  }, "The device could not be removed.");

  const nameOf = (id: string | null) => (!id ? "the node itself" : id.startsWith("passkey:") ? "a passkey" : devices?.find(d => d.id === id)?.name ?? "a removed device");
  const setupFlow = <>
    {naming !== null && <form className="ob-pairing-offer" aria-label="Set up Tor Browser" onSubmit={e => { e.preventDefault(); void browser(naming.trim()); }}>
      <div>
        <h3>Set up Tor Browser</h3>
        <label className="ob-field"><span>Name this browser</span>
          <input value={naming} onChange={e => setNaming(e.target.value)} autoComplete="off" autoFocus/>
        </label>
        <div className="ob-form-actions">
          <button type="submit" className="ob-primary" disabled={busy || !naming.trim()}>Create keys</button>
          <button type="button" className="ob-secondary" disabled={busy} onClick={() => setNaming(null)}>Cancel</button>
        </div>
      </div>
    </form>}
    {offer && <PairingOffer code={offer.code} expires={offer.expires} onDone={() => { setOffer(null); void refresh(); }}/>}
    {credential && <BrowserCredentialOffer credential={credential} onDone={() => setCredential(null)}/>}
  </>;
  return <div className={`ob-settings-card ${presentation === "desk" ? "ob-moderator-devices" : ""}`}>
    <div className="ob-settings-card-heading">
      <h2>{presentation === "desk" ? "Paired devices" : remoteTarget ? "Devices paired with your home node" : "Devices paired with this node"}</h2>
      <p>Each device holds its own key. Remove a lost or stolen device here; anything it paired is removed with it. Paying from the wallet still needs the wallet passphrase on every device.</p>
    </div>
    {status && !status.enabled && <p className="ob-settings-notice">Remote access is off on this node. Start it with <code>ob-node serve --remote --lan</code>, or turn on Run this computer as a home node, to pair devices.</p>}
    {status?.moving && <p className="ob-settings-notice">The node is moving to a new onion address. Paired devices switch over the next time they connect.</p>}
    {status?.onion && <CopyPaymentValue label="Onion address" value={`http://${status.onion}/`}/>}
    {pending.map(request => <div key={request.requestId} className="ob-pairing-offer" role="group" aria-label={`${request.name} wants to pair`}>
      <div>
        <h3>“{request.name}” ({PLATFORMS[request.platform] ?? request.platform}) wants to pair</h3>
        <Sas sas={request.sas} names={request.sasNames}/>
        <p>Approve only if the new device shows these same four emoji.</p>
        <div className="ob-form-actions">
          <button className="ob-primary" disabled={busy} onClick={() => void decide(request, true)}>Approve</button>
          <button className="ob-secondary" disabled={busy} onClick={() => void decide(request, false)}>Reject</button>
        </div>
      </div>
    </div>)}
    {devices === null ? null : devices.length === 0
      ? <div className="ob-settings-empty">No device is paired yet.</div>
      : <ul className="ob-labeler-list ob-device-list">{devices.map(device => <li key={device.id}>
          <div>
            <strong>{device.name}{device.thisDevice && <span className="ob-device-tag">This device</span>}</strong>
            <span>{PLATFORMS[device.platform] ?? device.platform} · paired {when(device.pairedAt)} by {nameOf(device.pairedBy)} · last seen {when(device.lastSeen)}{device.lastVia ? ` ${VIA[device.lastVia] ?? device.lastVia}` : ""} · push {device.push ? "on" : "off"}</span>
            <code>{device.id}</code>
          </div>
          <button className="ob-secondary" disabled={busy} onClick={() => void remove(device)}>Remove</button>
        </li>)}</ul>}
    {status?.enabled && <div className="ob-form-actions ob-device-actions">
      <button className="ob-primary" disabled={busy} onClick={() => void pairAnother()}>Pair a new device</button>
      {status.webShop && <button className="ob-secondary" disabled={busy} onClick={() => setNaming("Tor Browser")}>Set up Tor Browser</button>}
      <button className="ob-secondary" disabled={busy} onClick={() => void refresh()}>Refresh</button>
    </div>}
    {presentation === "desk" ? (naming !== null || offer || credential) && <Dialog label={credential ? "Browser access keys" : offer ? "Pair a new device" : "Set up Tor Browser"} onCancel={() => { if (!busy) { setNaming(null); setOffer(null); setCredential(null); } }}>
      {setupFlow}
      {error && <p className="ob-form-error" role="alert">{error}</p>}
    </Dialog> : setupFlow}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
  </div>;
}
