import { useState } from "react";
import {
  contactClassified,
  createClassified,
  deleteClassified,
  fetchClassifieds,
  fetchContactThreads,
  replyContact,
  type Classified,
  type ClassifiedCategory,
  type ClassifiedDraft,
  type ClassifiedKind,
  type ContactThread,
} from "../classifieds";
import { useNodeResource } from "../useNodeResource";
import { PageHeading } from "./PageHeading";
import { useLockedAction } from "./SellerKit";

const CATEGORIES: ClassifiedCategory[] = ["goods", "services", "housing", "jobs", "other"];

/** Browse, post, and contact on the node's WTB / WTS board. */
export function Classifieds() {
  const [kind, setKind] = useState<ClassifiedKind | "">("");
  const [category, setCategory] = useState<ClassifiedCategory | "">("");
  const [selected, setSelected] = useState<Classified | null>(null);
  const [thread, setThread] = useState<ContactThread | null>(null);
  const board = useNodeResource(
    () => fetchClassifieds({ kind: kind || undefined, category: category || undefined }),
    15000,
    true,
  );
  const inbox = useNodeResource(fetchContactThreads, 15000, true);

  return (
    <section className="ob-classifieds">
      <PageHeading title="Classifieds" />
      <p className="ob-lede">Want-to-buy and want-to-sell posts. Contact stays sealed to the poster — not order chat.</p>

      <div className="ob-field-grid">
        <label className="ob-field">
          <span>Kind</span>
          <select value={kind} onChange={event => setKind(event.target.value as ClassifiedKind | "")}>
            <option value="">All</option>
            <option value="wtb">Want to buy</option>
            <option value="wts">Want to sell</option>
          </select>
        </label>
        <label className="ob-field">
          <span>Category</span>
          <select value={category} onChange={event => setCategory(event.target.value as ClassifiedCategory | "")}>
            <option value="">All</option>
            {CATEGORIES.map(name => (
              <option key={name} value={name}>{name}</option>
            ))}
          </select>
        </label>
      </div>

      {board.error && <p className="ob-form-error" role="alert">{board.error}</p>}
      {board.loading && !board.data ? <p role="status">Loading classifieds…</p> : null}
      <div className="ob-orders">
        {(board.data ?? []).map(post => (
          <article key={post.id} className="ob-order-row">
            <button type="button" className="ob-linkish" onClick={() => { setSelected(post); setThread(null); }}>
              <strong>{post.kind.toUpperCase()}</strong> · {post.title}
            </button>
            <p>{post.category}{post.region ? ` · ${post.region}` : ""} · {post.lifeDays} days left</p>
          </article>
        ))}
        {!board.loading && board.data?.length === 0 ? <p className="ob-order-no-results">No live classifieds match.</p> : null}
      </div>

      <PostForm onPosted={() => void board.refresh()} />

      {selected && !thread ? (
        <PostDetail
          post={selected}
          onClose={() => setSelected(null)}
          onContacted={next => { setThread(next); void inbox.refresh(); }}
          onDeleted={() => { setSelected(null); void board.refresh(); }}
        />
      ) : null}

      {thread ? (
        <ThreadView
          thread={thread}
          onClose={() => setThread(null)}
          onUpdated={next => { setThread(next); void inbox.refresh(); }}
        />
      ) : null}

      <h2>Contact inbox</h2>
      {inbox.error && <p className="ob-form-error" role="alert">{inbox.error}</p>}
      <div className="ob-orders">
        {(inbox.data ?? []).map(item => (
          <article key={item.id} className="ob-order-row">
            <button type="button" className="ob-linkish" onClick={() => setThread(item)}>
              {item.title || "Contact"} · {item.asPoster ? "incoming" : "sent"}
            </button>
            <p>{item.messages.at(-1)?.text ?? "No messages yet."}</p>
          </article>
        ))}
        {!inbox.loading && inbox.data?.length === 0 ? <p className="ob-order-no-results">No contact threads yet.</p> : null}
      </div>
    </section>
  );
}

function PostForm({ onPosted }: { onPosted: () => void }) {
  const [draft, setDraft] = useState<ClassifiedDraft>({ kind: "wtb", category: "goods", title: "", body: "", lifeDays: 14 });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const { run, dialog } = useLockedAction(setError);

  return (
    <form
      className="ob-field-grid"
      onSubmit={event => {
        event.preventDefault();
        setError("");
        setBusy(true);
        void run(async () => {
          try {
            await createClassified(draft);
            setDraft({ kind: draft.kind, category: draft.category, title: "", body: "", lifeDays: 14 });
            onPosted();
          } finally {
            setBusy(false);
          }
        });
      }}
    >
      <h2>New post</h2>
      <label className="ob-field">
        <span>Kind</span>
        <select value={draft.kind} onChange={event => setDraft({ ...draft, kind: event.target.value as ClassifiedKind })}>
          <option value="wtb">Want to buy</option>
          <option value="wts">Want to sell</option>
        </select>
      </label>
      <label className="ob-field">
        <span>Category</span>
        <select value={draft.category} onChange={event => setDraft({ ...draft, category: event.target.value as ClassifiedCategory })}>
          {CATEGORIES.map(name => <option key={name} value={name}>{name}</option>)}
        </select>
      </label>
      <label className="ob-field ob-field-wide">
        <span>Title</span>
        <input value={draft.title} maxLength={120} onChange={event => setDraft({ ...draft, title: event.target.value })} />
      </label>
      <label className="ob-field ob-field-wide">
        <span>Body</span>
        <textarea value={draft.body} maxLength={4000} rows={4} onChange={event => setDraft({ ...draft, body: event.target.value })} />
      </label>
      <label className="ob-field">
        <span>Region (optional)</span>
        <input value={draft.region ?? ""} onChange={event => setDraft({ ...draft, region: event.target.value })} />
      </label>
      <label className="ob-field">
        <span>Days live</span>
        <input inputMode="numeric" value={String(draft.lifeDays ?? 14)} onChange={event => setDraft({ ...draft, lifeDays: Number(event.target.value) || 0 })} />
      </label>
      <div className="ob-form-actions"><button className="ob-primary" disabled={busy}>Post</button></div>
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      {dialog}
    </form>
  );
}

function PostDetail({
  post,
  onClose,
  onContacted,
  onDeleted,
}: {
  post: Classified;
  onClose: () => void;
  onContacted: (thread: ContactThread) => void;
  onDeleted: () => void;
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const { run, dialog } = useLockedAction(setError);

  return (
    <article className="ob-order-expanded">
      <h2>{post.title}</h2>
      <p>{post.kind.toUpperCase()} · {post.category}{post.region ? ` · ${post.region}` : ""}</p>
      <p>{post.body}</p>
      <form
        className="ob-field-grid"
        onSubmit={event => {
          event.preventDefault();
          setError("");
          setBusy(true);
          void run(async () => {
            try {
              onContacted(await contactClassified(post.id, text));
              setText("");
            } finally {
              setBusy(false);
            }
          });
        }}
      >
        <label className="ob-field ob-field-wide">
          <span>Contact</span>
          <textarea value={text} maxLength={2000} rows={3} onChange={event => setText(event.target.value)} />
        </label>
        <div className="ob-form-actions">
          <button className="ob-primary" disabled={busy}>Send</button>
          <button type="button" className="ob-secondary" onClick={onClose}>Close</button>
          <button
            type="button"
            className="ob-secondary"
            disabled={busy}
            onClick={() => {
              setError("");
              setBusy(true);
              void run(async () => {
                try {
                  await deleteClassified(post.id);
                  onDeleted();
                } finally {
                  setBusy(false);
                }
              });
            }}
          >
            Delete
          </button>
        </div>
      </form>
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      {dialog}
    </article>
  );
}

function ThreadView({
  thread,
  onClose,
  onUpdated,
}: {
  thread: ContactThread;
  onClose: () => void;
  onUpdated: (thread: ContactThread) => void;
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const { run, dialog } = useLockedAction(setError);

  return (
    <article className="ob-order-expanded">
      <h2>{thread.title || "Contact"}</h2>
      <div className="ob-messages">
        {thread.messages.map(message => (
          <p key={message.id}><strong>{message.from === "me" ? "You" : "Them"}:</strong> {message.text}</p>
        ))}
      </div>
      <form
        className="ob-field-grid"
        onSubmit={event => {
          event.preventDefault();
          setError("");
          setBusy(true);
          void run(async () => {
            try {
              onUpdated(await replyContact(thread.id, text));
              setText("");
            } finally {
              setBusy(false);
            }
          });
        }}
      >
        <label className="ob-field ob-field-wide">
          <span>Reply</span>
          <textarea value={text} maxLength={2000} rows={3} onChange={event => setText(event.target.value)} />
        </label>
        <div className="ob-form-actions">
          <button className="ob-primary" disabled={busy}>Send</button>
          <button type="button" className="ob-secondary" onClick={onClose}>Close</button>
        </div>
      </form>
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      {dialog}
    </article>
  );
}
