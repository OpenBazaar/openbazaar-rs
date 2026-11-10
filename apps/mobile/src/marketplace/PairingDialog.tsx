import { useEffect, useRef, useState, useSyncExternalStore, type FormEvent } from "react";
import { afterShopPair, onShopAwaitingCode, pairShop, shopAwaitingCode, shopPairingMessage, showPairingWith } from "../catalog";
import { Dialog, FormError, useSubmit } from "./WalletLock";

type Ask = { trade: (code: string) => Promise<void>; done: (paired: boolean) => void };

/// Asks for the one-time code the node printed. A click that needs pairing opens it.
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

/// Offers the dialog on every page while the shop has no pairing code.
export function PairingBanner() {
  const awaiting = useSyncExternalStore(onShopAwaitingCode, shopAwaitingCode);
  const { error, pair } = usePairShop();
  if (!awaiting && !error) return null;
  return <div className="ob-network-badge" role="status">
    <span className="ob-status-dot is-offline" aria-hidden="true"/>
    {error || "This browser is not paired with your node. Orders, wallet and settings need a pairing code."}
    <button className="ob-text-link" onClick={pair}>Enter pairing code</button>
  </div>;
}

/// The same button for a page that cannot load until the shop is paired.
export function PairButton() {
  const { error, pair } = usePairShop();
  return <>
    <button className="ob-primary" onClick={pair}>Enter pairing code</button>
    <FormError message={error}/>
  </>;
}

function PairingForm({ ask, onFinish }: { ask: Ask; onFinish: (paired: boolean) => void }) {
  const [code, setCode] = useState("");
  const { busy, error, run } = useSubmit();
  function submit(event: FormEvent) {
    event.preventDefault();
    void run(async () => {
      await ask.trade(code.trim());
      onFinish(true);
    }, "The shop could not be paired.");
  }
  return <form className="ob-wallet-lock" onSubmit={submit}>
    <h2>Pair this browser with your node</h2>
    <p>Your node printed a pairing code in its terminal. Type it here to place orders and open your wallet from this browser. The code works once and expires after 5 minutes.</p>
    <label className="ob-field"><span>Pairing code</span>
      <input value={code} onChange={event => setCode(event.target.value)} placeholder="XXXX-XXXX" autoComplete="off" autoCapitalize="characters" spellCheck={false} autoFocus/>
    </label>
    <FormError message={error}/>
    <div className="ob-form-actions">
      <button type="button" className="ob-secondary" disabled={busy} onClick={() => onFinish(false)}>Not now</button>
      <button type="submit" className="ob-primary" disabled={busy || !code.trim()}>{busy ? "Pairing…" : "Pair browser"}</button>
    </div>
  </form>;
}
