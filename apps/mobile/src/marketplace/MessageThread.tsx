import { useState, type FormEvent } from "react";
import type { OrderMessage } from "../messages";

function when(at: number): string {
  if (!at) return "";
  return new Date(at * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/**
 * The order's conversation. Messages are plain text end to end: this renders
 * them as text, and nothing in a message is ever fetched or followed.
 */
export function MessageThread({
  messages,
  viewer,
  otherLabel,
  onSend,
}: {
  messages: OrderMessage[];
  /** Which side the person looking at this thread is. */
  viewer: "buyer" | "seller";
  /** How the other side is named, e.g. "Buyer" or the store's name. */
  otherLabel: string;
  onSend: (text: string) => Promise<void>;
}) {
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function submit(e: FormEvent) {
    e.preventDefault();
    const text = draft.trim();
    if (!text || busy) return;
    setBusy(true);
    setError("");
    try {
      await onSend(text);
      setDraft("");
    } catch (err) {
      setError(err instanceof Error ? err.message : "The message could not be sent.");
    } finally {
      setBusy(false);
    }
  }

  return <section className="ob-messages" aria-label="Messages about this order">
    <h3>Messages</h3>
    {messages.length > 0 ? (
      <ol className="ob-message-list">
        {messages.map((message) => (
          <li key={message.id || `${message.at}-${message.text}`} className={message.from === viewer ? "is-mine" : ""}>
            <strong>{message.from === viewer ? "You" : otherLabel}</strong>
            <p>{message.text}</p>
            {message.at > 0 && <span>{when(message.at)}</span>}
          </li>
        ))}
      </ol>
    ) : (
      <p className="ob-messages-empty">No messages yet. Messages travel with this order, sealed between your two nodes.</p>
    )}
    <form className="ob-message-form" onSubmit={e => void submit(e)}>
      <label className="ob-field">
        <span>Write to {otherLabel.toLowerCase()}</span>
        <textarea value={draft} maxLength={2000} rows={3} onChange={e => setDraft(e.target.value)} placeholder="Message…"/>
      </label>
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      <div className="ob-form-actions"><button className="ob-primary" disabled={busy || !draft.trim()}>{busy ? "Sending…" : "Send message"}</button></div>
    </form>
  </section>;
}
