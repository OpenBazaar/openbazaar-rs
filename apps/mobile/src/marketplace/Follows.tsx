import { useState, type FormEvent } from "react";
import { fetchFollows, followStore, unfollowStore, type Follow } from "../seller";
import { useNodeResource } from "../useNodeResource";

/** Stores on other nodes that this node keeps a signed copy of. */
export function Follows({ onChanged }: { onChanged: (message: string) => void }) {
  const resource = useNodeResource(fetchFollows, 15000);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function change(task: () => Promise<Follow[]>, message: string) {
    setBusy(true);
    setError("");
    try {
      resource.update(await task());
      onChanged(message);
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : "The node could not change its followed stores.");
      return false;
    } finally {
      setBusy(false);
    }
  }
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!busy && url.trim() && (await change(() => followStore(url.trim()), "Following that store"))) setUrl("");
  }
  const follows = resource.data ?? [];
  return <div className="ob-settings-page">
    <header className="ob-settings-heading"><h1>Followed stores</h1><p>Add a store by its node's address. Your node checks every record the store signed and keeps a copy, so its listings stay visible while it is offline.</p></header>
    <form className="ob-follow-form" onSubmit={e => void submit(e)}>
      <label className="ob-field"><span>Store node address</span><input value={url} placeholder="http://127.0.0.1:8788" onChange={e => setUrl(e.target.value)}/></label>
      <button className="ob-primary" disabled={busy || !url.trim()}>{busy ? "Following…" : "Follow store"}</button>
    </form>
    {(error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    {resource.loading && !resource.data ? <p role="status">Loading followed stores…</p> : !follows.length ? <p className="ob-page-intro">You do not follow any stores yet.</p> : <ul className="ob-follow-list">{follows.map(follow => <li key={follow.url}>
      <span><strong>{follow.name ?? "Not reached yet"}</strong><small>{follow.url}</small><small>{follow.error ? follow.error : follow.storeId ? `${follow.listings} listing${follow.listings === 1 ? "" : "s"}${follow.fetched ? ` · checked ${new Date(follow.fetched * 1000).toLocaleTimeString()}` : ""}` : "Waiting for the store's node"}</small></span>
      <button className="ob-text-link" disabled={busy} onClick={() => void change(() => unfollowStore(follow.url), "Stopped following that store")}>Unfollow</button>
    </li>)}</ul>}
  </div>;
}
