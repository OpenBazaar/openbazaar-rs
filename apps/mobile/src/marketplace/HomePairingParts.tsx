import { useEffect, useState } from "react";
import QRCode from "qrcode";
import type { BrowserCredential, PairingCode } from "../homeNode/api";
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

export function PairingOffer({ code, expires, onDone }: { code: PairingCode; expires: number; onDone: () => void }) {
  const [qr, setQr] = useState("");
  const left = useCountdown(expires);
  useEffect(() => {
    let active = true;
    void QRCode.toDataURL(code.link, { width: 208, margin: 2, errorCorrectionLevel: "M", color: { dark: "#063853", light: "#ffffff" } }).then(src => { if (active) setQr(src); }, () => undefined);
    return () => { active = false; };
  }, [code.link]);
  return <div className="ob-pairing-offer" role="group" aria-label="New pairing code">
    {qr && <img src={qr} width={208} height={208} alt="Pairing code as a QR code"/>}
    <div>
      <h3>{left > 0 ? `Pair within ${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` : "This code has expired"}</h3>
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
