import { useEffect, useState } from "react";
import { fetchStoreModerators, type KnownModerator } from "../moderators";
import { ModeratorSummary, useModeratorStyles } from "./ModeratorSummary";

/** The moderators one store accepts, for the buyer to pick from. `value` is the chosen key. */
export function ModeratorChoice({ storeId, seller, value, onChange }: { storeId: string; seller: string; value: string; onChange: (id: string) => void }) {
  useModeratorStyles();
  const [list, setList] = useState<KnownModerator[] | null>(null);
  const [error, setError] = useState("");
  useEffect(() => { let active = true; fetchStoreModerators(storeId).then(next => { if (!active) return; setList(next); if (next.length && !next.some(moderator => moderator.id === value)) onChange(next[0].id); }, e => { if (active) setError(e instanceof Error ? e.message : "The store’s moderators could not be loaded."); }); return () => { active = false; }; }, [storeId]);
  if (error) return <p className="ob-form-error" role="alert">{error}</p>;
  if (!list) return <p className="ob-moderator-choice-note" role="status">Loading the moderators {seller} accepts…</p>;
  if (!list.length) return <p className="ob-form-error" role="alert">Your node holds no verified profile for a moderator {seller} accepts, so escrow cannot be used for this order.</p>;
  return <fieldset className="ob-moderator-choice">
    <legend>Choose a moderator for {seller}</legend>
    <p className="ob-moderator-choice-note">{seller} signed for each of these. The one you choose is written into the escrow terms, and only it can rule on a dispute.</p>
    {list.map(moderator => <label key={moderator.id} className={`ob-moderator-option ${moderator.id === value ? "is-selected" : ""}`}><input type="radio" name={`moderator-${seller}`} checked={moderator.id === value} onChange={() => onChange(moderator.id)}/><ModeratorSummary moderator={moderator}/></label>)}
  </fieldset>;
}
