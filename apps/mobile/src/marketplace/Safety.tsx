import { useState } from "react";
import type { ListingHeader } from "../catalog";
import { CONTENT_TAGS, parseKeywords, toggleHidden, type AdultMode, type SafetySettings, type SensitiveMode, type TagMode } from "../safety/filter";

export function SafetySettingsPanel({ settings, onChange }: { settings: SafetySettings; onChange: (settings: SafetySettings) => void }) {
  const [text, setText] = useState(() => settings.blockedKeywords.join("\n"));
  return <section className="ob-orders" aria-labelledby="ob-safety-title">
    <div className="ob-section-heading"><div><h1 id="ob-safety-title">Content filters</h1></div></div>
    <p className="ob-page-intro">These filters run on this device. They are not sent to your node or anyone else, and they do not remove anything from the network.</p>
    <label><input type="checkbox" checked={settings.safeMode} onChange={e => onChange({ ...settings, safeMode: e.target.checked })}/> Safe mode</label>
    <p className="ob-page-intro">Safe mode applies the rating and tag choices below, and treats unlabeled listings that use a short list of explicit words as sensitive.</p>
    <label className="ob-field">Sensitive listings<select aria-label="Sensitive listings" value={settings.sensitive} disabled={!settings.safeMode} onChange={e => onChange({ ...settings, sensitive: e.target.value as SensitiveMode })}><option value="hide">Hide</option><option value="cover">Cover, tap to show</option><option value="show">Show</option></select></label>
    <label className="ob-field">Adult listings<select aria-label="Adult listings" value={settings.adult} disabled={!settings.safeMode} onChange={e => onChange({ ...settings, adult: e.target.value as AdultMode })}><option value="hide">Hide</option><option value="show">Show</option></select></label>
    {CONTENT_TAGS.map(tag => <label key={tag} className="ob-field">Tagged {tag}<select aria-label={`Tagged ${tag}`} value={settings.tags[tag]} disabled={!settings.safeMode} onChange={e => onChange({ ...settings, tags: { ...settings.tags, [tag]: e.target.value as TagMode } })}><option value="show">Show</option><option value="hide">Hide</option></select></label>)}
    <label className="ob-field">Blocked words<textarea aria-label="Blocked words" rows={5} value={text} placeholder="One word or phrase per line" onChange={e => { setText(e.target.value); onChange({ ...settings, blockedKeywords: parseKeywords(e.target.value) }); }}/></label>
    <p className="ob-page-intro">Listings whose title, category, description, or tags contain one of these whole words are hidden, even with safe mode off.</p>
    <h2>Hidden</h2>
    <p className="ob-page-intro">Sellers and listings you hid stay hidden even with safe mode off. A seller is remembered by its store key, so a new name does not bring it back.</p>
    {settings.hiddenStores.length + settings.hiddenListings.length === 0 ? <p className="ob-page-intro">Nothing hidden.</p> : <ul>
      {settings.hiddenStores.map(item => <li key={item.id}>Seller {item.label} <button className="ob-text-link" onClick={() => onChange({ ...settings, hiddenStores: toggleHidden(settings.hiddenStores, item) })}>Unhide</button></li>)}
      {settings.hiddenListings.map(item => <li key={item.id}>Listing {item.label} <button className="ob-text-link" onClick={() => onChange({ ...settings, hiddenListings: toggleHidden(settings.hiddenListings, item) })}>Unhide</button></li>)}
    </ul>}
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
