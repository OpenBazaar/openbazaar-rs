import { useState, type FormEvent } from "react";
import type { Follow } from "../seller";
import { fetchAddressFollows, followAddress, followOutcome, followProblem, NOT_AN_ADDRESS, refusal, storeAddress, unfollowAddress } from "../storeAddress";
import { useNodeResource } from "../useNodeResource";

const FAILED = "The node could not change its followed stores.";

function state(follow: Follow): string {
  if (follow.error) return followProblem(follow.error);
  return follow.storeId ? `${follow.listings} listing${follow.listings === 1 ? "" : "s"}` : "Waiting for the store's node";
}

/** Stores followed at their node's address: no directory, the seller hands the address over. */
export function FollowAddress({ onChanged }: { onChanged: (message: string) => void }) {
  const resource = useNodeResource(fetchAddressFollows, 15000);
  const [pasted, setPasted] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy || !pasted.trim()) return;
    const url = storeAddress(pasted);
    if (!url) return setError(NOT_AN_ADDRESS);
    setBusy(true);
    setError("");
    try {
      const follows = await followAddress(url);
      resource.update(follows);
      setPasted("");
      const outcome = followOutcome(follows, url);
      if (outcome.ok) onChanged(outcome.message);
      else setError(outcome.message);
    } catch (e) {
      setError(e instanceof Error ? refusal(e.message) : FAILED);
    } finally {
      setBusy(false);
    }
  }
  async function remove(url: string) {
    setBusy(true);
    setError("");
    try {
      resource.update(await unfollowAddress(url));
      onChanged("Stopped following that store");
    } catch (e) {
      setError(e instanceof Error ? e.message : FAILED);
    } finally {
      setBusy(false);
    }
  }
  const follows = resource.data ?? [];
  return <section className="ob-settings-card ob-follow-address" aria-labelledby="ob-follow-address-title">
    <div className="ob-settings-card-heading"><h2 id="ob-follow-address-title">Follow a store by its address</h2><p>Paste a store address a seller handed you. Your node asks that node for the store, checks every record the store signed, and keeps a copy. No directory is asked.</p></div>
    <form className="ob-follow-form" onSubmit={e => void submit(e)}>
      <label className="ob-field"><span>Store address</span><input aria-label="Store address" value={pasted} spellCheck={false} autoCapitalize="off" placeholder="http://….onion" onChange={e => setPasted(e.target.value)}/></label>
      <button className="ob-primary" disabled={busy || !pasted.trim()}>{busy ? "Following…" : "Follow store"}</button>
    </form>
    {busy && <p className="ob-page-intro" role="status">Reaching the store. Over Tor this can take a minute.</p>}
    {(error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    {follows.length > 0 && <ul className="ob-follow-list">{follows.map(follow => <li key={follow.url}>
      <span><strong>{follow.name ?? "Not reached yet"}</strong><small>{follow.url}</small><small>{state(follow)}</small></span>
      <button className="ob-text-link" disabled={busy} onClick={() => void remove(follow.url)}>Remove</button>
    </li>)}</ul>}
  </section>;
}
