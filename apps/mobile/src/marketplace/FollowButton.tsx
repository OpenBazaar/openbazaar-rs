import { useState } from "react";
import { fetchDirectory, followNotice, followStore, unfollowStore } from "../follows";
import { useNodeResource } from "../useNodeResource";
import { Icon } from "./Marketplace";

/**
 * Follow or unfollow one store. It is drawn only for a store the node's
 * directory holds: the node says whether such a store is followed, and
 * following is what keeps it once the directory stops naming it.
 */
export function FollowButton({ storeId, onChanged }: { storeId: string; onChanged: (message: string) => void }) {
  const directory = useNodeResource(fetchDirectory, 0);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const store = directory.data?.find(item => item.storeId === storeId);
  // Unfollowing a store the directory does not name takes it off the list. The name is kept so that can be undone here.
  const listed = store ? store.name || "this store" : "";
  if (listed && listed !== name) setName(listed);
  if (!name) return null;
  const following = store?.added === true;
  async function toggle() {
    setBusy(true);
    setError("");
    try {
      const stores = await (following ? unfollowStore(storeId) : followStore(storeId));
      directory.update(stores);
      onChanged(followNotice(stores, storeId, name));
    } catch (e) {
      setError(e instanceof Error ? e.message : "The node could not change its followed stores.");
    } finally {
      setBusy(false);
    }
  }
  return <div className="ob-follow">
    <button type="button" className="ob-secondary" aria-pressed={following} aria-label={following ? `Following ${name}. Unfollow` : `Follow ${name}`} aria-busy={busy} disabled={busy} onClick={() => void toggle()}>
      {busy ? "Saving…" : following ? <><Icon name="check" size={16}/>Following<span className="ob-follow-undo">Unfollow</span></> : "Follow"}
    </button>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
  </div>;
}
