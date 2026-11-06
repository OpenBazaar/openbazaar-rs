import { useState } from "react";
import { setLabelers, type LabelerState } from "../catalog";

/** Follow or unfollow labelers by key. The node fetches and verifies their sets; this device decides what to do with the labels. */
export function LabelersPanel({ state, onState }: { state: LabelerState | null; onState: (state: LabelerState) => void }) {
  const [key, setKey] = useState("");
  const [url, setUrl] = useState("");
  const [status, setStatus] = useState("");
  const followed = state?.labelers ?? [];

  async function save(next: { key: string; url?: string | null }[]) {
    setStatus("Saving…");
    try {
      onState(await setLabelers(next));
      setStatus("");
      setKey("");
      setUrl("");
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "The labelers could not be saved.");
    }
  }

  const current = followed.map(l => ({ key: l.key, url: l.url }));
  const follow = (next: { key: string; url?: string | null }) => void save([...current.filter(l => l.key !== next.key), next]);
  const unfollow = (target: string) => void save(current.filter(l => l.key !== target));

  return <section>
    <h2>Labelers</h2>
    <p className="ob-page-intro">A labeler publishes signed labels for listings and stores. Follow one and safe mode treats its labels like a seller's own, using whichever is strictest. Labelers can only hide or cover things on this device. You follow none by default, and the list stays on your node.</p>
    {state === null ? <p className="ob-page-intro">Your node's labelers could not be read.</p> : null}
    {followed.length === 0 ? <p className="ob-page-intro">You follow no labelers.</p> : <ul>
      {followed.map(l => <li key={l.key}>
        {l.name ?? "Unknown labeler"} <code>{l.key.slice(0, 16)}…</code>{l.url ? ` from ${l.url}` : ""}{l.sequence !== null ? `, set ${l.sequence}` : ""}
        {l.error ? <> — {l.error}</> : null} <button className="ob-text-link" onClick={() => unfollow(l.key)}>Unfollow</button>
      </li>)}
    </ul>}
    {state?.samples.filter(s => !followed.some(l => l.key === s.key)).map(s => <p key={s.key} className="ob-page-intro">
      Sample: {s.name} <code>{s.key.slice(0, 16)}…</code> <button className="ob-text-link" onClick={() => follow({ key: s.key })}>Follow</button>
    </p>)}
    <label className="ob-field"><span>Labeler key</span><input value={key} onChange={e => setKey(e.target.value.trim())} placeholder="64 hex characters"/></label>
    <label className="ob-field"><span>Address (http on this machine)</span><input value={url} onChange={e => setUrl(e.target.value.trim())} placeholder="http://127.0.0.1:9000/labels"/></label>
    <button className="ob-primary" disabled={!key} onClick={() => follow({ key, url: url || null })}>Follow</button>
    {status ? <p className="ob-page-intro" role="status">{status}</p> : null}
  </section>;
}
