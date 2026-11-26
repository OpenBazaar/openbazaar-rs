import { useState, useRef, useEffect, type FormEvent, type KeyboardEvent } from "react";
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
  const listRef = useRef<HTMLOListElement>(null);

  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [messages.length]);

  async function submit(e?: FormEvent) {
    if (e) e.preventDefault();
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

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void submit();
    }
  }

  return (
    <section className="ob-messages" aria-label="Messages about this order">
      <h3>Messages</h3>
      {messages.length > 0 ? (
        <ol className="ob-message-list" ref={listRef}>
          {messages.map((message, index) => {
            const isMine = message.from === viewer;
            const prev = index > 0 ? messages[index - 1] : undefined;
            const isFirstInRun = !prev || prev.from !== message.from;
            const classes = [
              isMine ? "is-mine" : "is-theirs",
              isFirstInRun ? "is-first-in-group" : "is-consecutive",
            ].join(" ");

            return (
              <li key={message.id || `${message.at}-${message.text}`} className={classes}>
                {isMine ? (
                  isFirstInRun && <strong className="ob-sr-only">You</strong>
                ) : (
                  isFirstInRun && <strong className="ob-message-sender">{otherLabel}</strong>
                )}
                <p>{message.text}</p>
                {message.at > 0 && <span className="ob-message-time">{when(message.at)}</span>}
              </li>
            );
          })}
        </ol>
      ) : (
        <p className="ob-messages-empty">
          No messages yet. Messages travel with this order, sealed between your two nodes.
        </p>
      )}
      <form className="ob-message-form" onSubmit={e => void submit(e)}>
        <label className="ob-field">
          <span>Write to {otherLabel}</span>
          <textarea
            value={draft}
            maxLength={2000}
            rows={2}
            onChange={e => setDraft(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Message…"
          />
        </label>
        {error && <p className="ob-form-error" role="alert">{error}</p>}
        <div className="ob-form-actions">
          <span className="ob-message-hint">Enter to send, Shift+Enter for newline</span>
          <button className="ob-primary" type="submit" disabled={busy || !draft.trim()}>
            {busy ? "Sending…" : "Send"}
          </button>
        </div>
      </form>
    </section>
  );
}
