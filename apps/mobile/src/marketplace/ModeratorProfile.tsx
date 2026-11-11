import { useEffect, useState, type FormEvent } from "react";
import { fetchOwnModerator, languagesOf, saveOwnModerator, shortKey, type OwnModerator } from "../moderators";
import { isLocked } from "../walletLock";
import { useModeratorStyles } from "./ModeratorSummary";
import { CopyPaymentValue } from "./NodePayments";

type Fields = { name: string; description: string; terms: string; languages: string; feeSats: string };
const fieldsOf = (own: OwnModerator): Fields => ({ name: own.name, description: own.description, terms: own.terms, languages: own.languages.join(", "), feeSats: String(own.feeSats || "") });

/** The moderator's own identity: the key stores list, and the signed profile buyers and sellers read. */
export function ModeratorProfile({ unlocked, onUnlock }: { unlocked: boolean; onUnlock: () => void }) {
  useModeratorStyles();
  const [own, setOwn] = useState<OwnModerator | null>(null);
  const [fields, setFields] = useState<Fields | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  // The identity is made at the first unlock, so the profile is read again when the lock changes.
  useEffect(() => { let active = true; fetchOwnModerator().then(next => { if (!active) return; setOwn(next); setFields(current => current ?? fieldsOf(next)); setError(""); }, e => { if (active) setError(e instanceof Error ? e.message : "The moderator profile could not be loaded."); }); return () => { active = false; }; }, [unlocked]);
  const set = (key: keyof Fields) => (value: string) => { setNotice(""); setFields(f => f && { ...f, [key]: value }); };
  const fee = Number(fields?.feeSats);
  const feeError = own && fields && (!Number.isSafeInteger(fee) || fee < own.minFeeSats) ? `The fee is a whole number of at least ${own.minFeeSats.toLocaleString()} sats.` : "";
  async function save(e: FormEvent) {
    e.preventDefault();
    if (busy || !fields || feeError || !fields.name.trim()) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const next = await saveOwnModerator({ name: fields.name.trim(), description: fields.description.trim(), terms: fields.terms.trim(), languages: languagesOf(fields.languages), feeSats: fee });
      setOwn(next);
      setFields(fieldsOf(next));
      setNotice("Profile signed and published.");
    } catch (err) {
      if (isLocked(err)) onUnlock();
      setError(isLocked(err) ? "The wallet is locked. Unlock it, then save again." : err instanceof Error && err.message ? err.message : "The profile could not be saved.");
    } finally {
      setBusy(false);
    }
  }
  if (!own || !fields) return <section className="ob-moderator-profile" aria-label="Moderator profile">{error ? <p className="ob-form-error" role="alert">{error}</p> : <p role="status">Loading your moderator profile…</p>}</section>;
  return <section className="ob-moderator-profile" aria-label="Moderator profile">
    <div className="ob-moderator-identity">
      <h2>Your moderator identity</h2>
      {own.id ? <><p>Stores and buyers know this moderator by its key. The key comes from this node’s wallet, so no other node has it. The name below is a label and can be copied by anyone.</p><CopyPaymentValue label="Moderator key" value={own.id}/>{own.address ? <CopyPaymentValue label="Address to give sellers" value={own.address}/> : <p className="ob-moderator-inline-note">This node has no address other nodes can reach yet. It appears here once the node’s onion address is up.</p>}<p className={`ob-moderator-published ${own.published ? "is-on" : ""}`} role="status">{own.published ? `Published. ${own.rulings ? `${own.rulings} signed public ${own.rulings === 1 ? "ruling" : "rulings"} across ${own.stores} ${own.stores === 1 ? "store" : "stores"}.` : "No public rulings yet."}` : "Not published. Sellers cannot add this moderator until you save a name."}</p></> : <p>This node makes its moderator key the first time its wallet is unlocked. <button type="button" className="ob-text-link" onClick={onUnlock}>Unlock wallet</button></p>}
      <dl className="ob-moderator-tiers">
        <div><dt>Works while the wallet is locked</dt><dd>Serving your signed profile, receiving dispute letters, and checking them against the escrow terms and the chain.</dd></div>
        <div><dt>Needs the wallet unlocked</dt><dd>Changing this profile and signing a ruling. Both are signed with the moderator key.</dd></div>
      </dl>
      {own.builtIn && <p className="ob-moderator-inline-note">This node also answers for the built-in test moderator, {own.builtIn.name} ({shortKey(own.builtIn.id)}). Its keys are public, so it is for testnet4 coins only and has no track record.</p>}
    </div>
    {own.id && <form className="ob-moderator-profile-form" onSubmit={e => void save(e)}>
      <h2>Profile</h2>
      <div className="ob-field-grid">
        <label className="ob-field"><span>Display name</span><input value={fields.name} maxLength={64} required placeholder="How buyers and sellers see you" onChange={e => set("name")(e.target.value)}/></label>
        <label className="ob-field"><span>Fee per ruling (sats)</span><input value={fields.feeSats} inputMode="numeric" required aria-describedby="ob-moderator-fee-hint" onChange={e => set("feeSats")(e.target.value.replace(/\D/g, ""))}/></label>
        <label className="ob-field ob-field-wide"><span>Short description</span><input value={fields.description} maxLength={280} placeholder="What you handle and how fast you answer" onChange={e => set("description")(e.target.value)}/></label>
        <label className="ob-field"><span>Languages</span><input value={fields.languages} maxLength={120} placeholder="en, pt-br" onChange={e => set("languages")(e.target.value)}/></label>
        <label className="ob-field ob-field-wide"><span>Terms and policy</span><textarea value={fields.terms} maxLength={2000} placeholder="The evidence you ask for, how you decide, and when you answer." onChange={e => set("terms")(e.target.value)}/></label>
      </div>
      <p className="ob-field-hint" id="ob-moderator-fee-hint">A flat fee, paid from the escrow only when you rule. It is written into each escrow’s terms, so a change applies to new orders.</p>
      {feeError && <p className="ob-form-error" role="alert">{feeError}</p>}
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      {notice && <p className="ob-inline-notice" role="status">{notice}</p>}
      {!unlocked && <p className="ob-moderator-inline-note">The wallet is locked. <button type="button" className="ob-text-link" onClick={onUnlock}>Unlock it</button> to change the profile.</p>}
      <div className="ob-form-actions"><button className="ob-primary" disabled={busy || !unlocked || !!feeError || !fields.name.trim()}>{busy ? "Signing…" : own.published ? "Sign and publish changes" : "Sign and publish profile"}</button></div>
    </form>}
  </section>;
}
