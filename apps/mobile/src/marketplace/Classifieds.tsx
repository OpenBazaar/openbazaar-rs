import { useCallback, useEffect, useState } from "react";
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
import { Empty } from "./Marketplace";
import { PageHeading } from "./PageHeading";
import { useLockedAction } from "./SellerKit";
import { Dialog } from "./WalletLock";

const CATEGORIES: ClassifiedCategory[] = ["goods", "services", "housing", "jobs", "other"];
const PAGE_SIZES = [25, 50] as const;

function categoryLabel(name: string): string {
  return name ? name.charAt(0).toUpperCase() + name.slice(1) : name;
}

function kindLabel(kind: ClassifiedKind): string {
  return kind === "wtb" ? "Want to buy" : "Want to sell";
}

type Tab = "listings" | "inbox";

/** Browse, post, and contact on the node's WTB / WTS board. */
export function Classifieds() {
  const [tab, setTab] = useState<Tab>("listings");
  const [kind, setKind] = useState<ClassifiedKind | "">("");
  const [category, setCategory] = useState<ClassifiedCategory | "">("");
  const [query, setQuery] = useState("");
  const [region, setRegion] = useState("");
  const [listPage, setListPage] = useState(0);
  const [pageSize, setPageSize] = useState<number>(25);
  const [compose, setCompose] = useState(false);
  const [selected, setSelected] = useState<Classified | null>(null);
  const [thread, setThread] = useState<ContactThread | null>(null);
  const loadBoard = useCallback(
    () =>
      fetchClassifieds({
        kind: kind || undefined,
        category: category || undefined,
        q: query.trim() || undefined,
        region: region.trim() || undefined,
        limit: pageSize,
        offset: listPage * pageSize,
      }),
    [kind, category, query, region, pageSize, listPage],
  );
  const board = useNodeResource(loadBoard, 15000, true);
  const inbox = useNodeResource(fetchContactThreads, 15000, true);
  const threads = inbox.data ?? [];
  const posts = board.data?.classifieds ?? [];
  const total = board.data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / pageSize));
  const page = Math.min(listPage, pages - 1);
  const start = total === 0 ? 0 : page * pageSize;

  useEffect(() => {
    void board.refresh();
  }, [loadBoard, board.refresh]);

  function resetPage() {
    setListPage(0);
  }

  function changePage(next: number) {
    setListPage(Math.max(0, Math.min(next, pages - 1)));
  }

  function pagination(bottom = false) {
    return (
      <nav className="ob-order-pagination" aria-label={`Classifieds pagination${bottom ? " bottom" : ""}`}>
        <button type="button" aria-label="First page" disabled={page === 0} onClick={() => changePage(0)}>«</button>
        <button type="button" disabled={page === 0} onClick={() => changePage(page - 1)}>Previous</button>
        <span>Page {page + 1} of {pages.toLocaleString()}</span>
        <button type="button" disabled={page + 1 >= pages} onClick={() => changePage(page + 1)}>Next</button>
        <button type="button" aria-label="Last page" disabled={page + 1 >= pages} onClick={() => changePage(pages - 1)}>»</button>
      </nav>
    );
  }

  return (
    <section className="ob-classifieds">
      <PageHeading
        title="Classifieds"
        description="Want-to-buy and want-to-sell posts. Contact stays sealed to the poster — not order chat."
        action={<button type="button" className="ob-primary" onClick={() => setCompose(true)}>+ New post</button>}
      />

      <nav className="ob-classifieds-tabs" aria-label="Classifieds views">
        <button type="button" aria-current={tab === "listings" ? "page" : undefined} onClick={() => setTab("listings")}>Listings</button>
        <button type="button" aria-current={tab === "inbox" ? "page" : undefined} onClick={() => setTab("inbox")}>
          Contact inbox
          {threads.length > 0 ? <span className="ob-classifieds-tab-count">{threads.length}</span> : null}
        </button>
      </nav>

      {tab === "listings" ? (
        <>
          <div className="ob-classifieds-filters" role="search">
            <div className="ob-classifieds-segment" role="group" aria-label="Kind">
              {([["", "All", "All kinds"], ["wtb", "WTB", "Want to buy"], ["wts", "WTS", "Want to sell"]] as const).map(([value, label, title]) => (
                <button
                  key={value || "all"}
                  type="button"
                  title={title}
                  aria-pressed={kind === value}
                  onClick={() => { setKind(value); resetPage(); }}
                >
                  {label}
                </button>
              ))}
            </div>
            <label className="ob-classifieds-filter">
              <span className="ob-sr-only">Category</span>
              <select
                value={category}
                onChange={event => { setCategory(event.target.value as ClassifiedCategory | ""); resetPage(); }}
                aria-label="Category"
              >
                <option value="">All categories</option>
                {CATEGORIES.map(name => (
                  <option key={name} value={name}>{categoryLabel(name)}</option>
                ))}
              </select>
            </label>
            <label className="ob-classifieds-filter ob-classifieds-filter-grow">
              <span className="ob-sr-only">Search</span>
              <input
                value={query}
                onChange={event => { setQuery(event.target.value); resetPage(); }}
                placeholder="Search title or body"
                aria-label="Search"
              />
            </label>
            <label className="ob-classifieds-filter">
              <span className="ob-sr-only">Region</span>
              <input
                value={region}
                onChange={event => { setRegion(event.target.value); resetPage(); }}
                placeholder="Region"
                aria-label="Region"
              />
            </label>
            <label className="ob-classifieds-filter">
              <span className="ob-sr-only">Per page</span>
              <select
                aria-label="Classifieds per page"
                value={pageSize}
                onChange={event => { setPageSize(Number(event.target.value)); resetPage(); }}
              >
                {PAGE_SIZES.map(size => <option key={size} value={size}>{size} per page</option>)}
              </select>
            </label>
          </div>

          {board.error && <p className="ob-form-error" role="alert">{board.error}</p>}
          {board.loading && !board.data ? <p role="status">Loading classifieds…</p> : null}

          {!board.loading && total === 0 ? (
            <Empty
              icon="paper"
              title="No live classifieds match"
              text={kind || category || query.trim() || region.trim() ? "Try another kind, category, or search." : "Post a want-to-buy or want-to-sell notice. Contact stays sealed to the poster."}
            >
              <button type="button" className="ob-primary" onClick={() => setCompose(true)}>+ New post</button>
            </Empty>
          ) : total > 0 ? (
            <>
              <div className="ob-order-list-meta">
                <p role="status" aria-live="polite">
                  {(start + 1).toLocaleString()}–{Math.min(start + pageSize, total).toLocaleString()} of {total.toLocaleString()}{" "}
                  {total === 1 ? "classified" : "classifieds"}
                </p>
                {total > pageSize ? pagination() : null}
              </div>
              <ul className="ob-classifieds-list">
                {posts.map(post => (
                  <li key={post.id}>
                    <button type="button" className="ob-classifieds-card" onClick={() => { setSelected(post); setThread(null); }}>
                      <div className="ob-classifieds-card-meta">
                        <span className={`ob-classifieds-kind is-${post.kind}`}>{kindLabel(post.kind)}</span>
                        <span>{categoryLabel(post.category)}</span>
                        {post.region ? <span>{post.region}</span> : null}
                        <span>{post.lifeDays} days left</span>
                      </div>
                      <strong>{post.title}</strong>
                      <p>{post.body}</p>
                    </button>
                  </li>
                ))}
              </ul>
              {total > pageSize ? <div className="ob-order-pagination-footer">{pagination(true)}</div> : null}
            </>
          ) : null}
        </>
      ) : (
        <>
          {inbox.error && <p className="ob-form-error" role="alert">{inbox.error}</p>}
          {inbox.loading && !inbox.data ? <p role="status">Loading contact threads…</p> : null}
          {!inbox.loading && threads.length === 0 ? (
            <Empty icon="paper" title="No contact threads yet" text="Open a classified and send a sealed message. Replies show up here, separate from order chat." />
          ) : (
            <ul className="ob-classifieds-list">
              {threads.map(item => (
                <li key={item.id}>
                  <button type="button" className="ob-classifieds-card" onClick={() => setThread(item)}>
                    <div className="ob-classifieds-card-meta">
                      <span>{item.asPoster ? "Incoming" : "Sent"}</span>
                      <span>{item.messages.length} message{item.messages.length === 1 ? "" : "s"}</span>
                    </div>
                    <strong>{item.title || "Contact"}</strong>
                    <p>{item.messages.at(-1)?.text ?? "No messages yet."}</p>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}

      {compose ? (
        <ComposeDialog
          onClose={() => setCompose(false)}
          onPosted={() => {
            setCompose(false);
            setTab("listings");
            resetPage();
            void board.refresh();
          }}
        />
      ) : null}

      {selected && !thread ? (
        <PostDetail
          post={selected}
          onClose={() => setSelected(null)}
          onContacted={next => {
            setThread(next);
            setSelected(null);
            setTab("inbox");
            void inbox.refresh();
          }}
          onDeleted={() => {
            setSelected(null);
            void board.refresh();
          }}
        />
      ) : null}

      {thread ? (
        <ThreadView
          thread={thread}
          onClose={() => setThread(null)}
          onUpdated={next => {
            setThread(next);
            void inbox.refresh();
          }}
        />
      ) : null}
    </section>
  );
}

function ComposeDialog({ onClose, onPosted }: { onClose: () => void; onPosted: () => void }) {
  const [draft, setDraft] = useState<ClassifiedDraft>({ kind: "wtb", category: "goods", title: "", body: "", lifeDays: 14 });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const { run, dialog } = useLockedAction(setError);

  return (
    <Dialog label="New post" className="ob-classifieds-dialog" onCancel={busy ? () => undefined : onClose}>
      <form
        className="ob-classifieds-compose"
        onSubmit={event => {
          event.preventDefault();
          setError("");
          setBusy(true);
          void run(async () => {
            try {
              await createClassified(draft);
              onPosted();
            } finally {
              setBusy(false);
            }
          });
        }}
      >
        <header>
          <h2>New post</h2>
          <p>Signed by this node. Keep meeting details out of the public text.</p>
        </header>

        <div className="ob-classifieds-compose-row">
          <div className="ob-field">
            <span>Kind</span>
            <div className="ob-classifieds-segment" role="group" aria-label="Kind">
              <button type="button" aria-pressed={draft.kind === "wtb"} onClick={() => setDraft({ ...draft, kind: "wtb" })}>Want to buy</button>
              <button type="button" aria-pressed={draft.kind === "wts"} onClick={() => setDraft({ ...draft, kind: "wts" })}>Want to sell</button>
            </div>
          </div>
          <label className="ob-field">
            <span>Category</span>
            <select value={draft.category} onChange={event => setDraft({ ...draft, category: event.target.value as ClassifiedCategory })}>
              {CATEGORIES.map(name => <option key={name} value={name}>{categoryLabel(name)}</option>)}
            </select>
          </label>
        </div>

        <label className="ob-field">
          <span>Title</span>
          <input
            value={draft.title}
            maxLength={120}
            placeholder="What are you looking for or offering?"
            data-dialog-autofocus
            onChange={event => setDraft({ ...draft, title: event.target.value })}
          />
        </label>

        <label className="ob-field">
          <span>Body</span>
          <textarea
            value={draft.body}
            maxLength={4000}
            rows={5}
            placeholder="Short description. No street address, email, or phone."
            onChange={event => setDraft({ ...draft, body: event.target.value })}
          />
        </label>

        <div className="ob-classifieds-compose-row">
          <label className="ob-field">
            <span>Region (optional)</span>
            <input value={draft.region ?? ""} placeholder="Oregon" onChange={event => setDraft({ ...draft, region: event.target.value })} />
          </label>
          <label className="ob-field">
            <span>Days live</span>
            <div className="ob-classifieds-days">
              <input
                inputMode="numeric"
                value={String(draft.lifeDays ?? 14)}
                onChange={event => setDraft({ ...draft, lifeDays: Number(event.target.value) || 0 })}
              />
              <span>days</span>
            </div>
          </label>
        </div>

        {error && <p className="ob-form-error" role="alert">{error}</p>}
        <div className="ob-form-actions">
          <button type="button" className="ob-secondary" disabled={busy} onClick={onClose}>Cancel</button>
          <button type="submit" className="ob-primary" disabled={busy}>Post</button>
        </div>
      </form>
      {dialog}
    </Dialog>
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
    <Dialog label={post.title} className="ob-classifieds-dialog" onCancel={busy ? () => undefined : onClose}>
      <article className="ob-classifieds-detail">
        <header>
          <div className="ob-classifieds-card-meta">
            <span className={`ob-classifieds-kind is-${post.kind}`}>{kindLabel(post.kind)}</span>
            <span>{categoryLabel(post.category)}</span>
            {post.region ? <span>{post.region}</span> : null}
            <span>{post.lifeDays} days left</span>
          </div>
          <h2>{post.title}</h2>
          <p>{post.body}</p>
        </header>
        <form
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
          <label className="ob-field">
            <span>Contact</span>
            <textarea
              value={text}
              maxLength={2000}
              rows={3}
              placeholder="Write a sealed message to the poster"
              data-dialog-autofocus
              onChange={event => setText(event.target.value)}
            />
          </label>
          {error && <p className="ob-form-error" role="alert">{error}</p>}
          <div className="ob-form-actions">
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
            <button type="button" className="ob-secondary" disabled={busy} onClick={onClose}>Close</button>
            <button type="submit" className="ob-primary" disabled={busy}>Send</button>
          </div>
        </form>
        {dialog}
      </article>
    </Dialog>
  );
}

function when(at: number): string {
  if (!at) return "";
  return new Date(at * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
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
    <Dialog label={thread.title || "Contact"} className="ob-classifieds-dialog" onCancel={busy ? () => undefined : onClose}>
      <article className="ob-classifieds-detail">
        <header>
          <div className="ob-classifieds-card-meta">
            <span>{thread.asPoster ? "Incoming" : "Sent"}</span>
          </div>
          <h2>{thread.title || "Contact"}</h2>
        </header>
        <div className="ob-classifieds-messages">
          {thread.messages.map((message, index) => {
            const isMe = message.from === "me";
            const prev = index > 0 ? thread.messages[index - 1] : undefined;
            const isFirstInRun = !prev || prev.from !== message.from;
            const classes = [
              isMe ? "is-me" : "is-them",
              isFirstInRun ? "is-first-in-group" : "is-consecutive",
            ].join(" ");

            return (
              <p key={message.id} className={classes}>
                {isMe ? (
                  isFirstInRun && <strong className="ob-sr-only">You</strong>
                ) : (
                  isFirstInRun && <strong>Them</strong>
                )}
                <span>{message.text}</span>
                {message.at > 0 && <span className="ob-message-time">{when(message.at)}</span>}
              </p>
            );
          })}
        </div>
        <form
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
          <label className="ob-field">
            <span>Reply</span>
            <textarea
              value={text}
              maxLength={2000}
              rows={2}
              placeholder="Continue this sealed thread"
              data-dialog-autofocus
              onChange={event => setText(event.target.value)}
              onKeyDown={event => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  if (text.trim() && !busy) {
                    (event.currentTarget.form as HTMLFormElement | null)?.requestSubmit();
                  }
                }
              }}
            />
          </label>
          {error && <p className="ob-form-error" role="alert">{error}</p>}
          <div className="ob-form-actions">
            <span className="ob-message-hint">Enter to send, Shift+Enter for newline</span>
            <button type="button" className="ob-secondary" disabled={busy} onClick={onClose}>Close</button>
            <button type="submit" className="ob-primary" disabled={busy || !text.trim()}>
              {busy ? "Sending…" : "Send"}
            </button>
          </div>
        </form>
        {dialog}
      </article>
    </Dialog>
  );
}
