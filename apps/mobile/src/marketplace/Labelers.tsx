import { useState } from "react";
import { setLabelers, type LabelerState } from "../catalog";

/** Follow or unfollow labelers by key. The node fetches and verifies their sets; this device decides what to do with the labels. */
export function LabelersPanel({ state, onState }: { state: LabelerState | null; onState: (state: LabelerState) => void }) {
  const [key, setKey] = useState("");
  const [url, setUrl] = useState("");
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);
  const followed = state?.labelers ?? [];

  async function save(next: { key: string; url?: string | null }[]) {
    if (busy || !state) return;
    setBusy(true);
    setStatus("Saving…");
    try {
      onState(await setLabelers(next));
      setStatus("");
      setKey("");
      setUrl("");
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "The labelers could not be saved.");
    } finally {
      setBusy(false);
    }
  }

  const current = followed.map(l => ({ key: l.key, url: l.url }));
  const follow = (next: { key: string; url?: string | null }) => void save([...current.filter(l => l.key !== next.key), next]);
  const unfollow = (target: string) => void save(current.filter(l => l.key !== target));

  return <section className="ob-labelers-panel" aria-label="Label sources">
    <p>Labelers publish signed ratings for listings and stores. Safe mode uses the strictest rating from the seller and your followed sources. You follow none by default; this list is saved on your node.</p>
    {state === null ? <p className="ob-settings-notice" role="status">Your node’s label sources could not be read.</p> : null}
    {followed.length === 0 ? <div className="ob-settings-empty">You are not following any label sources.</div> : <ul className="ob-labeler-list">
      {followed.map(l => <li key={l.key}><div><strong>{l.name ?? "Unknown labeler"}</strong><code title={l.key}>{l.key}</code>{l.url && <span>{l.url}</span>}{l.sequence !== null && <span>Label set {l.sequence}</span>}{l.error && <p className="ob-form-error">{l.error}</p>}</div><button className="ob-secondary" disabled={busy} onClick={() => unfollow(l.key)}>Unfollow</button></li>)}
    </ul>}
    {state?.samples.filter(s => !followed.some(l => l.key === s.key)).map(s => <div key={s.key} className="ob-labeler-sample"><div><span className="ob-settings-footnote">Sample source</span><strong>{s.name}</strong><code title={s.key}>{s.key.slice(0, 16)}…</code></div><button className="ob-secondary" disabled={busy} onClick={() => follow({ key: s.key })}>Follow {s.name}</button></div>)}
    <form className="ob-labeler-form" onSubmit={e => { e.preventDefault(); if (/^[a-f0-9]{64}$/i.test(key)) follow({ key, url: url || null }); }}>
      <h3>Add a label source</h3><div className="ob-labeler-form-grid">
        <label className="ob-field"><span>Labeler key</span><input value={key} onChange={e => setKey(e.target.value.trim())} placeholder="64 hexadecimal characters" pattern="[a-fA-F0-9]{64}" required spellCheck={false} autoCapitalize="none"/></label>
        <label className="ob-field"><span>Source address <small>Optional</small></span><input value={url} onChange={e => setUrl(e.target.value.trim())} placeholder="http://127.0.0.1:9000/labels" type="url"/><small>HTTP on this machine.</small></label>
      </div><button className="ob-primary" disabled={!/^[a-f0-9]{64}$/i.test(key) || busy || !state}>Follow source</button>
    </form>
    {status ? <p className="ob-settings-footnote" role="status">{status}</p> : null}
  </section>;
}
