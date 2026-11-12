import { useEffect, useRef, useState, useSyncExternalStore, type FormEvent } from "react";
import { afterShopPair, onShopAwaitingCode, onThisComputer, pairShop, shopAwaitingCode, shopPairingMessage, showPairingWith } from "../catalog";
import { pairingBanner, pairingDialog, pairingKind, type PairingKind } from "../pairingUi";
import { Dialog, FormError, useSubmit } from "./WalletLock";

type Ask = { trade: (code: string) => Promise<void>; done: (paired: boolean) => void };

/// Asks for the one-time code, or Continue on this computer.
export function PairingDialog() {
  const [ask, setAsk] = useState<Ask | null>(null);
  const open = useRef<Ask | null>(null);
  open.current = ask;
  useEffect(() => {
    const forget = showPairingWith(trade => new Promise<boolean>(done => setAsk({ trade, done })));
    return () => {
      forget();
      open.current?.done(false);
    };
  }, []);
  if (!ask) return null;
  const finish = (paired: boolean) => {
    setAsk(null);
    ask.done(paired);
  };
  return <Dialog label="Pair this browser" onCancel={() => finish(false)}><PairingForm ask={ask} onFinish={finish}/></Dialog>;
}

/// Opens the dialog from a button. `error` is why the node could not ask for a code.
function usePairShop() {
  const [error, setError] = useState("");
  useEffect(() => afterShopPair(() => setError("")), []);
  function pair() {
    setError("");
    void pairShop().catch((e: unknown) => {
      if (e instanceof Error && shopPairingMessage(e.message)) return;
      setError(e instanceof Error ? e.message : "The shop could not be paired.");
    });
  }
  return { error, pair };
}

function voice(): PairingKind {
  return pairingKind(onThisComputer());
}

/// Offers the dialog on every page while the shop has no pairing code.
export function PairingBanner() {
  const awaiting = useSyncExternalStore(onShopAwaitingCode, shopAwaitingCode);
  const { error, pair } = usePairShop();
  const copy = pairingBanner(voice());
  if (!awaiting && !error) return null;
  return <div className="ob-network-badge" role="status">
    <span className="ob-status-dot is-offline" aria-hidden="true"/>
    {error || copy.text}
    <button className="ob-text-link" onClick={pair}>{copy.action}</button>
  </div>;
}

/// The same button for a page that cannot load until the shop is paired.
export function PairButton() {
  const { error, pair } = usePairShop();
  const copy = pairingBanner(voice());
  return <>
    <button className="ob-primary" onClick={pair}>{copy.action}</button>
    <FormError message={error}/>
  </>;
}

function PairingForm({ ask, onFinish }: { ask: Ask; onFinish: (paired: boolean) => void }) {
  const [kind, setKind] = useState<PairingKind>(voice);
  const [code, setCode] = useState("");
  const copy = pairingDialog(kind);
  const { busy, error, run } = useSubmit();
  function submit(event: FormEvent) {
    event.preventDefault();
    void run(async () => {
      try {
        await ask.trade(kind === "loopback" ? "" : code.trim());
        onFinish(true);
      } catch (err) {
        if (kind === "loopback") setKind("code");
        throw err;
      }
    }, "The shop could not be paired.");
  }
  return <form className="ob-wallet-lock" onSubmit={submit}>
    <h2>{copy.title}</h2>
    <p>{copy.body}</p>
    {kind === "code" && <label className="ob-field"><span>Pairing code</span>
      <input value={code} onChange={event => setCode(event.target.value)} placeholder="XXXX-XXXX" autoComplete="off" autoCapitalize="characters" spellCheck={false} autoFocus/>
    </label>}
    <FormError message={error}/>
    <div className="ob-form-actions">
      <button type="button" className="ob-secondary" disabled={busy} onClick={() => onFinish(false)}>Not now</button>
      <button type="submit" className="ob-primary" disabled={busy || (kind === "code" && !code.trim())}>{busy ? "Pairing…" : copy.action}</button>
    </div>
  </form>;
}
