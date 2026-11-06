import { useState } from "react";
import { parseKeywords, type SafetySettings } from "../safety/filter";

export function SafetySettingsPanel({ settings, onChange }: { settings: SafetySettings; onChange: (settings: SafetySettings) => void }) {
  const [text, setText] = useState(() => settings.blockedKeywords.join("\n"));
  return <section className="ob-orders" aria-labelledby="ob-safety-title">
    <div className="ob-section-heading"><div><h1 id="ob-safety-title">Content filters</h1></div></div>
    <p className="ob-page-intro">These filters run on this device. They are not sent to your node or anyone else, and they do not remove anything from the network.</p>
    <label><input type="checkbox" checked={settings.safeMode} onChange={e => onChange({ ...settings, safeMode: e.target.checked })}/> Safe mode</label>
    <p className="ob-page-intro">Hides listings the seller rated sensitive or adult, every listing from a store rated adult, and unlabeled listings that use a short list of explicit words.</p>
    <label className="ob-field">Blocked words<textarea aria-label="Blocked words" rows={5} value={text} placeholder="One word or phrase per line" onChange={e => { setText(e.target.value); onChange({ ...settings, blockedKeywords: parseKeywords(e.target.value) }); }}/></label>
    <p className="ob-page-intro">Listings whose title, category, description, or tags contain one of these whole words are hidden, even with safe mode off.</p>
  </section>;
}

export function SafetyInterstitial({ reason, onShow, onBack }: { reason: string; onShow: () => void; onBack: () => void }) {
  return <div className="ob-empty" role="alert">
    <h2>This listing is hidden on this device.</h2>
    <p>{reason} You can change this in Content filters.</p>
    <button className="ob-secondary" onClick={onShow}>Show it anyway</button>
    <button className="ob-primary" onClick={onBack}>Back to the goods</button>
  </div>;
}
