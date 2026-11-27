import { useState, type FormEvent } from "react";
import { followStore } from "../follows";
import { followAddress, followOutcome, refusal, storeAddress } from "../storeAddress";
import { Dialog } from "./WalletLock";

const STORE_KEY = /^[0-9a-f]{64}$/i;
const NOT_A_STORE = "That is not a store key or address. Paste 64 hex characters, or an address that starts with http:// and ends in .onion.";
const FAILED = "The node could not change its followed stores.";

/** Follow a store by its 64-hex key or its node's address. `onFollowed` says what happened; the dialog closes after it. */
export function FollowStoreDialog({ onClose, onFollowed }: { onClose: () => void; onFollowed: (message: string) => void }) {
  const [pasted, setPasted] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const text = pasted.trim();
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || !text) return;
    const url = STORE_KEY.test(text) ? null : storeAddress(text);
    if (!STORE_KEY.test(text) && !url) return setError(NOT_A_STORE);
    setBusy(true);
    setError("");
    try {
      if (url) {
        const outcome = followOutcome(await followAddress(url), url);
        if (!outcome.ok) return setError(outcome.message);
        onFollowed(outcome.message);
      } else {
        await followStore(text.toLowerCase());
        onFollowed("Store key saved. The node will keep its records.");
      }
      onClose();
    } catch (e) {
      setError(e instanceof Error ? refusal(e.message) : FAILED);
    } finally {
      setBusy(false);
    }
  }
  return <Dialog label="Follow a store" className="ob-directory-dialog" onCancel={busy ? () => undefined : onClose}>
    <form className="ob-directory-follow-form" onSubmit={event => void submit(event)}>
      <header>
        <h2>Follow a store</h2>
        <p>Paste the store key or the address a seller handed you. Your node keeps the store's signed records and checks each one.</p>
      </header>
      <label className="ob-field">
        <span>Store key or address</span>
        <input data-dialog-autofocus aria-label="Store key or address" value={pasted} spellCheck={false} autoCapitalize="off" autoComplete="off" placeholder="64 hex characters, or http://….onion" onChange={event => { setPasted(event.target.value); setError(""); }}/>
      </label>
      {busy && <p className="ob-page-intro" role="status">Reaching the store. Over Tor this can take a minute.</p>}
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      <div className="ob-form-actions">
        <button type="button" className="ob-secondary" disabled={busy} onClick={onClose}>Cancel</button>
        <button type="submit" className="ob-primary" disabled={busy || !text}>{busy ? "Following…" : "Follow store"}</button>
      </div>
    </form>
  </Dialog>;
}
