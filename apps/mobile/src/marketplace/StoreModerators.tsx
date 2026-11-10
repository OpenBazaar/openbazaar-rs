import { useEffect, useState, type FormEvent } from "react";
import { acceptModerator, fetchModerators, removeModerator, type KnownModerator } from "../moderators";
import { ModeratorSummary, useModeratorStyles } from "./ModeratorSummary";
import { useLockedAction } from "./SellerKit";

/** The moderators this store accepts. The list is signed into the store record, so every change needs the wallet. */
export function StoreModerators({ escrow, ownListings }: { escrow: boolean; ownListings: boolean }) {
  useModeratorStyles();
  const [list, setList] = useState<KnownModerator[] | null>(null);
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const { run, dialog } = useLockedAction(setError);
  useEffect(() => { if (!escrow) return; let active = true; fetchModerators().then(next => { if (active) setList(next); }, e => { if (active) setError(e instanceof Error ? e.message : "The moderators could not be loaded."); }); return () => { active = false; }; }, [escrow]);
  async function change(key: string, task: () => Promise<KnownModerator[]>, done: string) {
    if (busy) return;
    setBusy(key);
    setError("");
    setNotice("");
    await run(async () => { setList(await task()); setNotice(done); if (key === "add") setAddress(""); });
    setBusy("");
  }
  const add = (e: FormEvent) => { e.preventDefault(); if (address.trim()) void change("add", () => acceptModerator({ address: address.trim() }), "Moderator added. Your store record was signed again."); };
  const accepted = list?.filter(moderator => moderator.accepted) ?? [];
  const others = list?.filter(moderator => !moderator.accepted) ?? [];
  return <section className="ob-store-section ob-store-moderators" aria-labelledby="ob-store-moderators-title">
    <h2 id="ob-store-moderators-title">Moderators</h2>
    {!escrow ? <p className="ob-field-hint">Moderated escrow runs on the test network only, so there are no moderators to choose on mainnet.</p> : <>
      <p className="ob-field-hint">A buyer who pays into moderated escrow picks one of the moderators you accept here. The list is signed into your store record. A moderator is its key, not its name: confirm the key with the moderator before you accept it.</p>
      {!ownListings && <p className="ob-field-hint">Your own listings do not take escrow orders in this client yet. Until they do, this list applies to the test-network sample stores this node sells for.</p>}
      {!list ? !error && <p role="status">Loading moderators…</p> : <>
        {accepted.length ? <ul className="ob-moderator-list" aria-label="Accepted moderators">{accepted.map(moderator => <li key={moderator.id}><ModeratorSummary moderator={moderator}/><button type="button" className="ob-text-link" disabled={!!busy} aria-label={`Remove ${moderator.name || "moderator"}`} onClick={() => void change(moderator.id, () => removeModerator(moderator.id), "Moderator removed. New orders cannot choose it.")}>{busy === moderator.id ? "Removing…" : "Remove"}</button></li>)}</ul> : <p className="ob-moderator-none" role="status">You accept no moderators, so your store does not offer moderated escrow.</p>}
        {others.length > 0 && <><h3>Other moderators this node knows</h3><ul className="ob-moderator-list" aria-label="Known moderators">{others.map(moderator => <li key={moderator.id}><ModeratorSummary moderator={moderator}/><button type="button" className="ob-secondary" disabled={!!busy} aria-label={`Accept ${moderator.name || "moderator"}`} onClick={() => void change(moderator.id, () => acceptModerator({ id: moderator.id }), "Moderator accepted. Your store record was signed again.")}>{busy === moderator.id ? "Signing…" : "Accept"}</button></li>)}</ul></>}
      </>}
      <form className="ob-moderator-add" onSubmit={add}>
        <label className="ob-field"><span>Add a moderator by address</span><input value={address} maxLength={96} placeholder="http://…onion" autoComplete="off" spellCheck={false} onChange={e => setAddress(e.target.value)}/></label>
        <button className="ob-secondary" disabled={!!busy || !address.trim()}>{busy === "add" ? "Checking profile…" : "Add moderator"}</button>
      </form>
      <p className="ob-field-hint">Your node fetches the moderator’s signed profile from that address over Tor and checks the signature before it is listed.</p>
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      {notice && <p className="ob-inline-notice" role="status">{notice}</p>}
    </>}
    {dialog}
  </section>;
}
