import { useState, type FormEvent } from "react";
import { paired } from "../catalog";
import { useNodeResource } from "../useNodeResource";

type DirectoryStore = { storeId: string; name: string | null; listings: number; added: boolean };

function storesOf(value: unknown): DirectoryStore[] {
  if (!value || typeof value !== "object" || !("stores" in value)) return [];
  const rows = (value as { stores?: unknown }).stores;
  if (!Array.isArray(rows)) return [];
  return rows.flatMap(row => {
    if (!row || typeof row !== "object" || !("storeId" in row)) return [];
    const item = row as { storeId?: unknown; name?: unknown; listings?: unknown; added?: unknown };
    if (typeof item.storeId !== "string") return [];
    return [{
      storeId: item.storeId,
      name: typeof item.name === "string" ? item.name : null,
      listings: typeof item.listings === "number" ? item.listings : 0,
      added: item.added === true,
    }];
  });
}

function fetchDirectory(): Promise<DirectoryStore[]> {
  return paired("/directory").then(storesOf);
}

/** Stores this node already has, by name. A pasted key is the only way to add one. */
export function Follows({ onChanged }: { onChanged: (message: string) => void }) {
  const resource = useNodeResource(fetchDirectory, 15000);
  const [storeId, setStoreId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const key = storeId.trim();
  const ready = /^[0-9a-fA-F]{64}$/.test(key);
  async function change(task: () => Promise<DirectoryStore[]>, message: string) {
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
    if (busy || !ready) return;
    const saved = await change(
      () => paired("/follow", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ storeId: key }) }).then(storesOf),
      "Store key saved. The node will keep its records.",
    );
    if (saved) setStoreId("");
  }
  const stores = resource.data ?? [];
  return <div className="ob-settings-page">
    <header className="ob-settings-heading"><h1>Followed stores</h1><p>Stores the directory has named show up in Products. Paste a store key someone handed you. This node keeps that store's signed records and sends an order to the onion inside the record.</p></header>
    <form className="ob-follow-form" onSubmit={e => void submit(e)}>
      <label className="ob-field"><span>Store key</span><input aria-label="Store key" value={storeId} spellCheck={false} autoCapitalize="off" placeholder="64 hex characters" onChange={e => setStoreId(e.target.value)}/></label>
      <button className="ob-primary" disabled={busy || !ready}>{busy ? "Adding…" : "Add store"}</button>
    </form>
    {(error || resource.error) && <p className="ob-form-error" role="alert">{error || resource.error}</p>}
    {resource.loading && !resource.data ? <p role="status">Loading stores…</p> : !stores.length ? <p className="ob-page-intro">No stores yet. A store the directory names shows up here, and in Products, after the next check.</p> : <ul className="ob-follow-list">{stores.map(store => <li key={store.storeId}>
      <span><strong>{store.name ?? "Waiting for the store's record"}</strong><small>{store.listings} listing{store.listings === 1 ? "" : "s"}</small></span>
      {store.added ? <button className="ob-text-link" disabled={busy} onClick={() => void change(() => paired("/follow", { method: "DELETE", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ storeId: store.storeId }) }).then(storesOf), "Stopped following that store")}>Remove</button> : null}
    </li>)}</ul>}
  </div>;
}
