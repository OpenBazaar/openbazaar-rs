import { useEffect, useState } from "react";
import QRCode from "qrcode";
import type { BrowserCredential, PairingCode, PairingOnion } from "../homeNode/api";
import { CopyPaymentValue } from "./NodePayments";

/** The four emoji both screens show while a device pairs. */
export function Sas({ sas, names }: { sas: string[]; names: string[] }) {
  return <ol className="ob-sas" aria-label={`Check emoji: ${names.join(", ")}`}>
    {sas.map((emoji, i) => <li key={i}><span aria-hidden="true">{emoji}</span><small>{names[i]}</small></li>)}
  </ol>;
}

function useCountdown(expires: number): number {
  const [left, setLeft] = useState(() => Math.max(0, Math.round((expires - Date.now()) / 1000)));
  useEffect(() => {
    const timer = setInterval(() => setLeft(Math.max(0, Math.round((expires - Date.now()) / 1000))), 1000);
    return () => clearInterval(timer);
  }, [expires]);
  return left;
}

const clock = (secs: number) => `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;

function useElapsed(since: number): number {
  const [secs, setSecs] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setSecs(Math.max(0, Math.floor((Date.now() - since) / 1000))), 1000);
    return () => clearInterval(timer);
  }, [since]);
  return secs;
}

/**
 * A new code, once a phone can reach it. Over Tor a code is reached at an onion address made for it, which takes a while to
 * appear: until the node has reached that address itself (`onion` is "publishing") the QR and words are held back, unless the
 * link also works on the node's local network. "slow": the node gave up waiting, and the code is shown with a warning.
 */
export function PairingOffer({ code, expires, onion, since, onDone }: { code: PairingCode; expires: number; onion: PairingOnion; since: number; onDone: () => void }) {
  const [qr, setQr] = useState("");
  const left = useCountdown(expires);
  const waited = useElapsed(since);
  useEffect(() => {
    let active = true;
    void QRCode.toDataURL(code.link, { width: 208, margin: 2, errorCorrectionLevel: "M", color: { dark: "#063853", light: "#ffffff" } }).then(src => { if (active) setQr(src); }, () => undefined);
    return () => { active = false; };
  }, [code.link]);
  if (onion === "publishing" && !code.lan) return <div className="ob-pairing-offer" role="group" aria-label="New pairing code">
    <div>
      <h3>Publishing a private address for this code… {clock(waited)}</h3>
      <p role="status">Your node gives each pairing code its own private Tor address, and a new address takes a little while to appear on the Tor network. The node is publishing it now and checking that it answers. The QR code and the eight words will show here as soon as it does, usually within a minute. The code's ten minutes start then.</p>
      <button className="ob-text-link" onClick={onDone}>Cancel</button>
    </div>
  </div>;
  return <div className="ob-pairing-offer" role="group" aria-label="New pairing code">
    {qr && <img src={qr} width={208} height={208} alt="Pairing code as a QR code"/>}
    <div>
      <h3>{left > 0 ? `Pair within ${clock(left)}` : "This code has expired"}</h3>
      {onion === "slow" && <p role="note">This code's private Tor address is taking unusually long to appear. You can scan the code now: the phone keeps trying for up to five minutes and may need a few of them. If it gives up, make a new code.</p>}
      {onion === "publishing" && <p role="note">On the same network as your node this code works now. Anywhere else the phone reaches it over Tor, and your node is still publishing the private address for that ({clock(waited)}).</p>}
      <p>On the other device choose Connect to my home node, then scan this code or type the eight words. Both screens then show four emoji; approve the device below only if they match.</p>
      <ol className="ob-pair-words">{code.words.map((word, i) => <li key={i}>{word}</li>)}</ol>
      <CopyPaymentValue label="Pairing link" value={code.link}/>
      <button className="ob-text-link" onClick={onDone}>Done</button>
    </div>
  </div>;
}

export function BrowserCredentialOffer({ credential, onDone }: { credential: BrowserCredential; onDone: () => void }) {
  return <div className="ob-pairing-offer" role="group" aria-label="Tor Browser credential">
    <div>
      <h3>Set up Tor Browser</h3>
      <p>Open the address in Tor Browser. When it asks for a key to the onion service, paste the onion key. Then sign in to the shop with the access key. Both are shown only now; keep them in a password manager. Anyone with both can manage your store until you remove this browser.</p>
      {credential.onion && <CopyPaymentValue label="Address" value={`http://${credential.onion}/`}/>}
      <CopyPaymentValue label="Onion key" value={credential.discoverySecret}/>
      <CopyPaymentValue label="Access key" value={credential.accessKey}/>
      <button className="ob-text-link" onClick={onDone}>Done</button>
    </div>
  </div>;
}
