import { useState, type FormEvent } from "react";
import { canRecuse, partyName, quoteRecusal, recuseDispute, type Dispute, type RefundQuote } from "../moderator";
import { Dialog, FormError, Passphrase, useSubmit } from "./WalletLock";

type Pending = { quote: RefundQuote; title: string; note: string; confirm: (quote: string, passphrase: string) => Promise<unknown> };

/** Paying a filing fee back from this node's wallet: a recusal before any decision. */
export function CaseRefunds({ letters, all, unlocked, onDone }: { letters: Dispute[]; all: Dispute[]; unlocked: boolean; onDone: () => void }) {
  const [pending, setPending] = useState<Pending | null>(null);
  const { busy, error, run } = useSubmit();
  const paid = letters.find(letter => canRecuse(letter, all));
  if (!paid) return null;
  const recuse = () => void run(async () => setPending({
    quote: await quoteRecusal(paid),
    title: "Recuse from this case",
    note: "The recusal is final and public. No letter of this case can be ruled on or denied afterwards; the escrow follows its timelocks.",
    confirm: (quote, passphrase) => recuseDispute(paid, quote, passphrase),
  }), "The refund could not be quoted.");
  return <section className="ob-case-refunds" aria-label="Refunds">
    <p className="ob-page-intro">Recusing pays the {partyName(paid.fee?.paidBy ?? paid.favors)}’s filing fee back on-chain from this node’s wallet. This node pays the network fee.</p>
    <FormError message={pending ? "" : error}/>
    <button className="ob-text-link" disabled={!unlocked || busy} onClick={recuse}>{busy && !pending ? "Checking…" : "Recuse and refund the fee"}</button>
    {pending && <RefundDialog pending={pending} onCancel={() => setPending(null)} onDone={() => { setPending(null); onDone(); }}/>}
  </section>;
}

function RefundDialog({ pending, onCancel, onDone }: { pending: Pending; onCancel: () => void; onDone: () => void }) {
  const [passphrase, setPassphrase] = useState("");
  const { busy, error, run } = useSubmit();
  const { quote } = pending;
  return <Dialog label={pending.title} onCancel={onCancel}>
    <form className="ob-spend-confirm" onSubmit={(e: FormEvent) => { e.preventDefault(); void run(async () => { await pending.confirm(quote.quote, passphrase); onDone(); }, "Nothing was paid back."); }}>
      <h2>{pending.title}</h2>
      <dl className="ob-spend-terms">
        <dt>Pay back</dt><dd><strong>{quote.amountSats.toLocaleString()} sats</strong> to the {partyName(quote.filer)}</dd>
        {quote.address && <><dt>Address</dt><dd>{quote.address}</dd></>}
        <dt>Network fee</dt><dd>{quote.feeSats.toLocaleString()} sats, at most {quote.feeCapSats.toLocaleString()}</dd>
        <dt>Total</dt><dd>{quote.totalSats.toLocaleString()} sats from this node’s wallet</dd>
      </dl>
      <p className="ob-page-intro">{pending.note} The address is the one in the party’s own letter. This confirmation expires in {quote.expiresInSecs} seconds.</p>
      <Passphrase label="Wallet passphrase" value={passphrase} onChange={setPassphrase} autoFocus/>
      <FormError message={error}/>
      <div className="ob-form-actions">
        <button className="ob-secondary" type="button" onClick={onCancel}>Cancel</button>
        <button className="ob-primary" disabled={busy || !passphrase}>{busy ? "Paying back…" : "Confirm refund"}</button>
      </div>
    </form>
  </Dialog>;
}
