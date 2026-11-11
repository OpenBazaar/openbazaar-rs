import { useRef, useState } from "react";
import { servedByHomeNode, signOut } from "../homeNode/transport";
import { disputeState, fetchDisputes, partyName, ruleDispute, type Dispute } from "../moderator";
import { useNodeResource } from "../useNodeResource";
import { isLocked, lockWallet } from "../walletLock";
import { HomeDevicesCard } from "./HomeDevicesCard";
import { ModeratorProfile } from "./ModeratorProfile";
import { CopyPaymentValue } from "./NodePayments";
import { PasskeysCard } from "./PasskeysCard";
import { Dialog, useLockState, WalletLockPanel } from "./WalletLock";

const failed = (e: unknown, fallback: string) => e instanceof Error && e.message ? e.message : fallback;
type Case = { id: string; letters: Dispute[]; resolved: boolean };
function casesOf(disputes: Dispute[]): Case[] {
  const groups = new Map<string, Dispute[]>();
  for (const letter of disputes) groups.set(letter.id, [...(groups.get(letter.id) ?? []), letter]);
  return [...groups].map(([id, letters]) => ({ id, letters, resolved: letters.some(letter => letter.ruled) }));
}
const shortId = (id: string) => `${id.slice(0, 8)}…${id.slice(-6)}`;

/** A case workspace. Wallet administration and device access are secondary views. */
export function ModeratorDesk({ onShop }: { onShop: () => void }) {
  const lock = useLockState();
  const [section, setSection] = useState<"disputes" | "profile" | "access">("disputes");
  const resource = useNodeResource(fetchDisputes, 15000, section === "disputes");
  const [filter, setFilter] = useState("open");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [security, setSecurity] = useState(false);
  const [error, setError] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [locking, setLocking] = useState(false);
  const [proposal, setProposal] = useState<Dispute | null>(null);
  const [ruling, setRuling] = useState(false);
  const submitting = useRef(false);
  const [dark, setDark] = useState(() => document.documentElement.dataset.obTheme === "dark");
  const cases = casesOf(resource.data ?? []);
  const visible = cases.filter(item => filter === "all" || (filter === "resolved" ? item.resolved : !item.resolved));
  const selected = visible.find(item => item.id === selectedId) ?? visible[0];
  const openCount = cases.filter(item => !item.resolved).length;
  const unlocked = lock.data?.state === "unlocked" && !lock.error;
  const proposed = proposal && resource.data?.find(letter => letter.id === proposal.id && letter.favors === proposal.favors);
  const canRule = !!proposed && disputeState(proposed, resource.data ?? []) === "open" && unlocked && !resource.error && !resource.loading;

  async function refresh() {
    if (refreshing) return;
    setRefreshing(true);
    try { await Promise.all([resource.refresh(), lock.refresh()]); }
    finally { setRefreshing(false); }
  }
  async function lockNow() {
    if (locking) return;
    setLocking(true);
    setError("");
    try { lock.update(await lockWallet()); }
    catch (err) { setError(failed(err, "The wallet could not be locked.")); }
    finally { setLocking(false); }
  }
  async function confirmRuling() {
    if (!proposal || !canRule || submitting.current) return;
    submitting.current = true;
    setRuling(true);
    setError("");
    try {
      await ruleDispute(proposal);
      setProposal(null);
    } catch (err) {
      if (isLocked(err)) await lock.refresh();
      setError(isLocked(err) ? "The wallet locked. Unlock it before issuing a ruling." : failed(err, "The ruling was not sent."));
    } finally {
      await resource.refresh();
      submitting.current = false;
      setRuling(false);
    }
  }
  function appearance() {
    const next = !dark;
    document.documentElement.dataset.obTheme = next ? "dark" : "light";
    try { localStorage.setItem("ob-theme", next ? "dark" : "light"); } catch { /* Appearance can still change for this session. */ }
    setDark(next);
  }

  const workspaceActions = <><button className="ob-secondary" onClick={onShop}>Open marketplace</button>{servedByHomeNode() && <button className="ob-text-link" onClick={() => { signOut(); location.reload(); }}>Sign out</button>}<button className="ob-moderator-theme" role="switch" aria-label="Dark mode" aria-checked={dark} onClick={appearance}>{dark ? "Light mode" : "Dark mode"}</button></>;
  return <main className="ob-desk ob-moderator-workspace" id="ob-content">
    <div className="ob-moderator-inner">
      <header className="ob-moderator-topbar">
        <div className="ob-moderator-brand"><img src={dark ? "brand/ob-market-logo-256.png" : "brand/ob-market-logo-light.png"} width={34} height={34} alt=""/><strong>OpenBazaar<span>Moderator workspace</span></strong></div>
        <div className="ob-moderator-top-actions is-desktop">{workspaceActions}</div>
        <details className="ob-moderator-mobile-menu"><summary>Menu</summary><div>{workspaceActions}<button className="ob-secondary" onClick={() => setSecurity(true)}>Wallet security</button></div></details>
      </header>
      <header className="ob-moderator-heading"><div><h1>Moderator desk</h1><p>Review escrow disputes and issue a final ruling.</p></div><button className="ob-secondary" onClick={() => setSecurity(true)}>Wallet security</button></header>
      <div className="ob-moderator-status">
        <div><span className={`ob-status-dot ${unlocked ? "" : "is-loading"}`} aria-hidden="true"/><strong>{lock.error ? "Wallet status unavailable" : !lock.data ? "Checking wallet…" : unlocked ? "Ready to sign rulings" : lock.data.state === "setup" ? "Wallet setup required" : "Wallet locked"}</strong><span>{unlocked ? "Rulings require this node’s signing key." : "You can review cases while locked."}</span></div>
        {unlocked ? <button className="ob-text-link" disabled={locking} onClick={() => void lockNow()}>{locking ? "Locking…" : "Lock wallet"}</button> : <button className="ob-text-link" onClick={() => setSecurity(true)}>{lock.data?.state === "setup" ? "Set up wallet" : "Unlock wallet"}</button>}
      </div>
      {lock.data && !lock.data.backupConfirmed && <p className="ob-moderator-backup">Backup not confirmed. <button className="ob-text-link" onClick={() => setSecurity(true)}>Review backup</button></p>}
      {lock.error && <p className="ob-form-error" role="alert">{lock.error}</p>}
      {error && !proposal && <p className="ob-form-error" role="alert">{error}</p>}
      <nav className="ob-moderator-nav" aria-label="Moderator sections">
        <button aria-current={section === "disputes" ? "page" : undefined} onClick={() => setSection("disputes")}>Disputes{resource.data && <span>{openCount} open</span>}</button>
        <button aria-current={section === "profile" ? "page" : undefined} onClick={() => setSection("profile")}>Profile</button>
        <button aria-current={section === "access" ? "page" : undefined} onClick={() => setSection("access")}>Access & devices</button>
      </nav>
      {section === "access" ? <section className="ob-moderator-access" aria-label="Access and devices"><PasskeysCard/><HomeDevicesCard presentation="desk"/></section> : section === "profile" ? <ModeratorProfile unlocked={unlocked} onUnlock={() => setSecurity(true)}/> : <section className="ob-moderator-disputes" aria-label="Dispute workspace">
        <header className="ob-case-toolbar"><div role="group" aria-label="Filter cases">{["open", "resolved", "all"].map(value => <button key={value} aria-pressed={filter === value} onClick={() => setFilter(value)}>{value === "open" ? "Open" : value === "resolved" ? "Resolved" : "All cases"}{resource.data && <span>{value === "open" ? openCount : value === "resolved" ? cases.length - openCount : cases.length}</span>}</button>)}</div><button className="ob-secondary" disabled={refreshing || resource.loading} onClick={() => void refresh()}>{refreshing ? "Refreshing…" : "Refresh disputes"}</button></header>
        {resource.error && <p className="ob-form-error" role="alert">{resource.error} Previous results may be out of date.</p>}
        {resource.loading && !resource.data ? <p className="ob-case-empty" role="status">Loading disputes…</p> : !resource.data || (resource.error && resource.data.length === 0) ? <div className="ob-case-empty"><h2>Disputes unavailable</h2><p>Check the connection to this node, then refresh.</p></div> : !visible.length ? <div className="ob-case-empty"><span className="ob-case-empty-mark" aria-hidden="true">—</span><h2>{filter === "open" ? "No disputes awaiting review" : "No cases in this view"}</h2><p>{cases.length ? "Choose another filter to see the other cases." : "Disputes appear here after this node checks their escrow terms, signatures and funding."}</p></div> : <div className={`ob-case-workspace ${visible.length > 1 ? "has-multiple" : ""}`}>
          <aside className="ob-case-queue" aria-label="Case queue"><p>{visible.length} {visible.length === 1 ? "case" : "cases"}</p><ul>{visible.map(item => <li key={item.id}><button aria-pressed={selected?.id === item.id} onClick={() => setSelectedId(item.id)}><strong>Case {shortId(item.id)}</strong><span>{item.letters.length} {item.letters.length === 1 ? "letter" : "letters"} · {item.resolved ? "Resolved" : "Awaiting review"}</span><small>{item.letters.map(letter => partyName(letter.favors)).join(" · ")}</small></button></li>)}</ul></aside>
          {selected && <article className="ob-case-detail" aria-labelledby="ob-case-title"><header><div><span className="ob-case-eyebrow">Escrow dispute{selected.letters[0].builtIn ? " · built-in test moderator" : ""}</span><h2 id="ob-case-title">Case {shortId(selected.id)}</h2></div><span className={`ob-case-state ${selected.resolved ? "is-resolved" : ""}`}>{selected.resolved ? "Resolved" : "Awaiting review"}</span></header>
            <p className="ob-case-verification">The node checked the submitted letters, escrow terms and on-chain funding. Review each party’s statement before deciding.</p>
            <details className="ob-case-identifiers"><summary>Escrow and funding references</summary><CopyPaymentValue label="Escrow ID" value={selected.id}/>{[...new Set(selected.letters.map(letter => letter.outpoint))].map(outpoint => <CopyPaymentValue key={outpoint} label="Funding outpoint" value={outpoint}/>)}</details>
            <div className="ob-case-letters">{selected.letters.map(letter => {
              const state = disputeState(letter, resource.data ?? []);
              return <section className="ob-case-letter" key={`${letter.id}-${letter.favors}`} aria-label={`${partyName(letter.favors)} statement`}><header><h3>{letter.favors === "buyer" ? "Buyer requests a refund" : "Seller requests payment"}</h3><span>Received at block {letter.receivedHeight.toLocaleString()}</span></header><p className="ob-case-memo">{letter.memo || "No statement was included in this letter."}</p>
                <footer>{state === "open" ? <button className="ob-secondary" disabled={!unlocked || !!resource.error || ruling} onClick={() => { setError(""); setProposal(letter); }}>Review ruling for {partyName(letter.favors)}</button> : <strong className="ob-case-outcome">{state === "granted" ? `Ruled for the ${partyName(letter.favors)}` : "Closed by the ruling for the other party"}</strong>}</footer>
              </section>;
            })}</div>
            {!unlocked && !selected.resolved && <p className="ob-moderator-inline-note">Unlock the wallet using the control above before issuing a ruling.</p>}
            {!selected.resolved && <p className="ob-case-final-note">A final ruling applies to this escrow and closes both parties’ letters.</p>}
          </article>}
        </div>}
        <p className="ob-moderator-poll-note">Updates every 15 seconds while this view is visible.</p>
      </section>}
      {security && <Dialog label="Wallet security" onCancel={() => setSecurity(false)}><h2>Wallet security</h2><WalletLockPanel lock={lock} purpose="moderator"/><div className="ob-form-actions"><button className="ob-secondary" onClick={() => setSecurity(false)}>Done</button></div></Dialog>}
      {proposal && <Dialog label="Confirm final ruling" onCancel={() => { if (!ruling) setProposal(null); }}><h2>Confirm final ruling</h2><p>Rule for the <strong>{partyName(proposal.favors)}</strong> in case <strong>{shortId(proposal.id)}</strong>?</p><p>{proposal.favors === "buyer" ? "The buyer can claim the escrowed funds back." : "The seller can claim the escrowed funds."} This ruling cannot be changed and closes the other party’s letter.</p><p className="ob-moderator-inline-note">This action signs a final ruling with this node’s wallet.</p>{!canRule && <p className="ob-form-error" role="alert">{!unlocked ? "The wallet must be unlocked to confirm this ruling." : "This case changed or its status could not be verified. Refresh disputes before continuing."}</p>}{error && <p className="ob-form-error" role="alert">{error}</p>}<div className="ob-form-actions"><button className="ob-secondary" disabled={ruling} onClick={() => setProposal(null)}>Cancel</button><button className="ob-primary" disabled={ruling || !canRule} onClick={() => void confirmRuling()}>{ruling ? "Signing ruling…" : `Confirm ruling for ${partyName(proposal.favors)}`}</button></div></Dialog>}
    </div>
  </main>;
}
