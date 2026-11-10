import { useState, type FormEvent } from "react";
import { displayNameError, setVacation, updateProfile, type ProfileChanges, type SellerProfile } from "../seller";
import { PageHeading } from "./PageHeading";
import { RailMark } from "./RailMark";
import { ImageUpload, MediaImage, useLockedAction } from "./SellerKit";

type Fields = Required<Omit<ProfileChanges, "shipsTo">> & { shipsTo: string };

function fieldsOf(profile: SellerProfile): Fields {
  return {
    name: profile.name,
    region: profile.region,
    tagline: profile.tagline,
    about: profile.about,
    avatar: profile.avatar,
    banner: profile.banner,
    shippingPolicy: profile.shippingPolicy,
    returnsPolicy: profile.returnsPolicy,
    processingTime: profile.processingTime,
    shipsTo: profile.shipsTo.join(", "),
  };
}

function TextField({ label, value, onChange, max, wide = false, multiline = false, placeholder }: { label: string; value: string; onChange: (value: string) => void; max: number; wide?: boolean; multiline?: boolean; placeholder?: string }) {
  return <label className={`ob-field ${wide ? "ob-field-wide" : ""}`}>
    <span>{label}</span>
    {multiline ? <textarea value={value} maxLength={max} placeholder={placeholder} onChange={e => onChange(e.target.value)}/> : <input value={value} maxLength={max} placeholder={placeholder} onChange={e => onChange(e.target.value)}/>}
  </label>;
}

/** The storefront editor: what buyers see, signed by the node when saved. */
export function StoreSetup({ profile, onSaved, onViewStore }: { profile: SellerProfile; onSaved: (profile: SellerProfile, message: string) => void; onViewStore: () => void }) {
  const [fields, setFields] = useState(() => fieldsOf(profile));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const { run, dialog } = useLockedAction(setError);
  const set = <K extends keyof Fields>(key: K) => (value: Fields[K]) => setFields(f => ({ ...f, [key]: value }));
  const first = !profile.setUp;
  const nameError = displayNameError(fields.name);
  async function pause(away: boolean) {
    if (busy) return;
    setBusy(true);
    setError("");
    await run(async () => {
      const next = await setVacation(away);
      onSaved(next, next.away ? "Your store is paused" : "Your store is taking orders again");
    });
    setBusy(false);
  }
  async function save(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    if (nameError) return;
    setBusy(true);
    setError("");
    const shipsTo = fields.shipsTo.split(/[\s,]+/).map(code => code.trim().toUpperCase()).filter(Boolean);
    await run(async () => {
      const next = await updateProfile({ ...fields, shipsTo });
      setFields(fieldsOf(next));
      onSaved(next, first ? "Your store is open" : "Store saved and signed");
    });
    setBusy(false);
  }
  return <section className="ob-store-setup">
    <PageHeading title={first ? "Set up your store" : "Store settings"} description="Manage store identity, customer policies, shipping coverage, and payment methods." action={profile.setUp ? <button className="ob-secondary" onClick={onViewStore}>View your store</button> : undefined}/>
    <form className="ob-store-form" onSubmit={e => void save(e)}>
      <div className="ob-store-images">
        <div className="ob-store-banner-edit">
          {fields.banner ? <MediaImage hash={fields.banner} alt="Store banner" className="ob-store-banner-img"/> : <span className="ob-store-banner-empty">No banner</span>}
          <div className="ob-store-image-actions">
            <ImageUpload label={fields.banner ? "Change banner" : "Add banner"} onUploaded={set("banner")}/>
            {fields.banner && <button type="button" className="ob-text-link" onClick={() => set("banner")(null)}>Remove</button>}
          </div>
        </div>
        <div className="ob-store-avatar-edit">
          <span className="ob-store-avatar">{fields.avatar ? <MediaImage hash={fields.avatar} alt="Store avatar"/> : (fields.name.charAt(0) || "?")}</span>
          <ImageUpload label={fields.avatar ? "Change avatar" : "Add avatar"} onUploaded={set("avatar")}/>
          {fields.avatar && <button type="button" className="ob-text-link" onClick={() => set("avatar")(null)}>Remove</button>}
        </div>
      </div>
      <fieldset className="ob-store-section">
        <legend>Store identity</legend>
        <div className="ob-field-grid">
          <TextField label="Display name" value={fields.name} onChange={set("name")} max={64} placeholder="Brook and Pine"/>
        </div>
        <p className="ob-field-hint">Buyers see this name on your listings. It is the store's public name.</p>
        <p className="ob-saved-name" role="status">Saved name: {profile.name}</p>
        {nameError && <p className="ob-form-error" role="alert">{nameError}</p>}
      </fieldset>
      <fieldset className="ob-store-section">
        <legend>About your store</legend>
        <div className="ob-field-grid">
          <TextField label="Based in" value={fields.region} onChange={set("region")} max={40} placeholder="Region or country"/>
          <TextField label="Tagline" value={fields.tagline} onChange={set("tagline")} max={120} wide placeholder="One line about what you make or sell"/>
          <TextField label="Description" value={fields.about} onChange={set("about")} max={2000} wide multiline/>
        </div>
      </fieldset>
      <fieldset className="ob-store-section">
        <legend>Shipping and returns</legend>
        <div className="ob-field-grid">
          <TextField label="Ships to (country codes)" value={fields.shipsTo} onChange={set("shipsTo")} max={400} placeholder="US, CA, GB"/>
          <TextField label="Processing time" value={fields.processingTime} onChange={set("processingTime")} max={120} placeholder="Ships in 2–3 days"/>
          <TextField label="Shipping policy" value={fields.shippingPolicy} onChange={set("shippingPolicy")} max={500} multiline placeholder="Delivery coverage, tracking, customs and duties, and lost or damaged shipments."/>
          <TextField label="Returns policy" value={fields.returnsPolicy} onChange={set("returnsPolicy")} max={500} multiline placeholder="Return window, eligibility, item condition, return postage, refunds and exclusions."/>
        </div>
      </fieldset>
      <fieldset className="ob-store-section">
        <legend>Payments</legend>
        <ul className="ob-payment-rails">
          <li className="is-on"><strong className="ob-rail"><RailMark rail="bitcoin" />Bitcoin</strong><span>{profile.network === "testnet4" ? "Testnet4 wallet. " : ""}Each accepted order receives a fresh payment address. New sellers must satisfy the node's escrow and reputation requirements before accepting orders.</span></li>
          <li className={profile.xmrAvailable ? "" : "is-off"}><strong className="ob-rail"><RailMark rail="monero" />Monero</strong><span>{profile.xmrAvailable ? "Buyers can pay in Monero." : "Not available for your own store on this node yet."}</span></li>
          <li className={profile.acceptsLightning ? "is-on" : "is-off"}><strong className="ob-rail"><RailMark rail="lightning" />Lightning</strong><span>{profile.acceptsLightning ? "Buyers can pay a Lightning invoice. Direct pay only." : "Turn it on in Lightning settings. It stays off on mainnet until connections run over Tor."}</span></li>
          <li className="is-off"><strong className="ob-rail"><RailMark rail="zcash" />Zcash</strong><span>Demo stores on this node take Zcash in Zcash settings. Your store takes it once sales from other nodes carry Zcash terms.</span></li>
          <li className={profile.escrowAvailable ? "" : "is-off"}><strong>Moderated escrow</strong><span>{profile.escrowAvailable ? "Buyers can choose a moderator." : "Merchant escrow setup is not available in this client. New-seller orders that require escrow cannot be accepted yet."}</span></li>
        </ul>
      </fieldset>
      {profile.setUp ? <fieldset className="ob-store-section">
        <legend>Vacation</legend>
        <p className="ob-field-hint">Pause the store. Listings stay up with no stock, and new orders are refused. Buyers are not told where you are.</p>
        <button type="button" className="ob-secondary" disabled={busy} onClick={() => void pause(!profile.away)}>{profile.away ? "End vacation" : "Pause the store"}</button>
      </fieldset> : null}
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      <div className="ob-form-actions">
        <button className="ob-primary" disabled={busy || nameError !== null}>{busy ? "Signing…" : first ? "Open my store" : "Save store"}</button>
      </div>
    </form>
    {dialog}
  </section>;
}
