import { useState, type ReactNode } from "react";
import type { ListingHeader } from "../catalog";
import { CONTENT_TAGS, parseKeywords, toggleHidden, type AdultMode, type SafetySettings, type SensitiveMode } from "../safety/filter";

export function SafetySettingsPanel({ settings, onChange, labelSources }: { settings: SafetySettings; onChange: (settings: SafetySettings) => void; labelSources: ReactNode }) {
  const [text, setText] = useState(() => settings.blockedKeywords.join("\n"));
  const hiddenCount = settings.hiddenStores.length + settings.hiddenListings.length;
  return <section className="ob-settings-page" aria-labelledby="ob-safety-title">
    <header className="ob-settings-heading"><div className="ob-settings-eyebrow">Browsing preferences</div><h1 id="ob-safety-title">Content filters</h1><p>Choose what appears in your marketplace. Changes apply automatically.</p></header>
    <div className="ob-filter-privacy"><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/></svg><span>Your browsing filters stay on this device. Nothing is removed from the network.</span></div>
    <section className={`ob-settings-card ob-safe-mode ${settings.safeMode ? "is-enabled" : ""}`} aria-labelledby="ob-safe-mode-title">
      <div><div className="ob-setting-title"><h2 id="ob-safe-mode-title">Safe mode</h2><span className="ob-setting-state">{settings.safeMode ? "On" : "Off"}</span></div><p id="ob-safe-mode-help">Apply your rating and tag rules, plus a built-in filter for explicit words in unrated listings.</p></div>
      <label className="ob-setting-switch"><input type="checkbox" role="switch" aria-label="Safe mode" aria-describedby="ob-safe-mode-help" checked={settings.safeMode} onChange={e => onChange({ ...settings, safeMode: e.target.checked })}/><span aria-hidden="true"/></label>
    </section>
    {!settings.safeMode && <p className="ob-settings-notice" role="status">Rating and tag rules are paused. Blocked words and hidden items still apply.</p>}
    <div className="ob-settings-grid">
      <section className="ob-settings-card" aria-labelledby="ob-ratings-title"><header className="ob-settings-card-heading"><h2 id="ob-ratings-title">Content ratings</h2><p>Choose how listings with these ratings appear.</p></header>
        <div className="ob-setting-row"><label htmlFor="ob-sensitive"><strong>Sensitive content</strong><span>Hide it, cover it until opened, or show it.</span></label><select id="ob-sensitive" aria-label="Sensitive listings" value={settings.sensitive} disabled={!settings.safeMode} onChange={e => onChange({ ...settings, sensitive: e.target.value as SensitiveMode })}><option value="hide">Hide</option><option value="cover">Cover</option><option value="show">Show</option></select></div>
        <div className="ob-setting-row"><label htmlFor="ob-adult"><strong>Adult content</strong><span>Listings rated for adults.</span></label><select id="ob-adult" aria-label="Adult listings" value={settings.adult} disabled={!settings.safeMode} onChange={e => onChange({ ...settings, adult: e.target.value as AdultMode })}><option value="hide">Hide</option><option value="show">Show</option></select></div>
        <p className="ob-settings-footnote">General listings are shown unless another filter applies.</p>
      </section>
      <section className="ob-settings-card" aria-labelledby="ob-tags-title"><header className="ob-settings-card-heading"><h2 id="ob-tags-title">Content tags</h2><p>Hide listings with any tag you switch on.</p></header>
        <div className="ob-tag-controls">{CONTENT_TAGS.map(tag => <label key={tag} className="ob-tag-setting"><span>{tag.charAt(0).toUpperCase() + tag.slice(1)}</span><span className="ob-setting-switch"><input type="checkbox" role="switch" aria-label={`Hide ${tag}`} checked={settings.tags[tag] === "hide"} disabled={!settings.safeMode} onChange={e => onChange({ ...settings, tags: { ...settings.tags, [tag]: e.target.checked ? "hide" : "show" } })}/><span aria-hidden="true"/></span></label>)}</div>
      </section>
    </div>
    <section className="ob-settings-card ob-keyword-card" aria-labelledby="ob-keywords-title"><div><h2 id="ob-keywords-title">Blocked words</h2><p>Hide listings that contain these words or phrases in their title, description, category, or tags.</p><p className="ob-settings-footnote">Matches whole words. Always applies, even with safe mode off.</p></div><div className="ob-keyword-input"><label htmlFor="ob-blocked-words">One word or phrase per line</label><textarea id="ob-blocked-words" aria-label="Blocked words" rows={4} value={text} placeholder={"Add words or phrases…"} onChange={e => { setText(e.target.value); onChange({ ...settings, blockedKeywords: parseKeywords(e.target.value) }); }}/><span className="ob-settings-footnote">{settings.blockedKeywords.length} {settings.blockedKeywords.length === 1 ? "phrase" : "phrases"} blocked</span></div></section>
    <details className="ob-settings-card ob-settings-disclosure"><summary><span><strong>Hidden sellers & listings</strong><span>Review items you have hidden and bring them back.</span></span><span className="ob-settings-count">{hiddenCount}</span><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg></summary><div className="ob-settings-disclosure-body"><p>These stay hidden even with safe mode off. Sellers are remembered by their store key, so changing a name does not bring them back.</p>
      {hiddenCount === 0 ? <div className="ob-settings-empty">No hidden sellers or listings.</div> : <ul className="ob-hidden-list">{settings.hiddenStores.map(item => <li key={item.id}><div><strong>{item.label}</strong><span>Seller</span></div><button className="ob-secondary" onClick={() => onChange({ ...settings, hiddenStores: toggleHidden(settings.hiddenStores, item) })} aria-label={`Unhide seller ${item.label}`}>Unhide</button></li>)}{settings.hiddenListings.map(item => <li key={item.id}><div><strong>{item.label}</strong><span>Listing</span></div><button className="ob-secondary" onClick={() => onChange({ ...settings, hiddenListings: toggleHidden(settings.hiddenListings, item) })} aria-label={`Unhide listing ${item.label}`}>Unhide</button></li>)}</ul>}
    </div></details>
    <details className="ob-settings-card ob-settings-disclosure"><summary><span><strong>Label sources</strong><span>Use content ratings published by labelers you choose.</span></span><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg></summary><div className="ob-settings-disclosure-body">{labelSources}</div></details>
  </section>;
}

export function HideActions({ listing, settings, onChange }: { listing: ListingHeader; settings: SafetySettings; onChange: (settings: SafetySettings) => void }) {
  return <div className="ob-hide-actions">
    <button className="ob-text-link" onClick={() => onChange({ ...settings, hiddenListings: toggleHidden(settings.hiddenListings, { id: listing.hash, label: listing.title }) })}>Hide this listing</button>
    {" · "}
    <button className="ob-text-link" onClick={() => onChange({ ...settings, hiddenStores: toggleHidden(settings.hiddenStores, { id: listing.storeId, label: listing.seller }) })}>Hide this seller</button>
  </div>;
}

export function SafetyInterstitial({ reason, onShow, onBack }: { reason: string; onShow: () => void; onBack: () => void }) {
  return <div className="ob-empty" role="alert">
    <h2>This listing is hidden on this device.</h2>
    <p>{reason} You can change this in Content filters.</p>
    <button className="ob-secondary" onClick={onShow}>Show it anyway</button>
    <button className="ob-primary" onClick={onBack}>Back to the goods</button>
  </div>;
}

export function CoveredCard({ reason, onShow }: { reason: string; onShow: () => void }) {
  return <article className="ob-product-card ob-covered-card">
    <p>Covered by your content filters. {reason}</p>
    <button className="ob-secondary" onClick={onShow}>Show</button>
  </article>;
}
