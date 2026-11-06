import { useState } from "react";
import { setContentLabel, type ContentRating, type ShopCatalog } from "../catalog";
import { CONTENT_TAGS } from "../safety/filter";

type Rating = Exclude<ContentRating, "unrated">;
const RATINGS: Rating[] = ["general", "sensitive", "adult"];

/** The vendor's own rating for the store and each listing. The node signs the change with the store key it holds. */
export function VendorLabelPicker({ catalog, onCatalog }: { catalog: ShopCatalog; onCatalog: (catalog: ShopCatalog) => void }) {
  const [target, setTarget] = useState("store");
  const current = target === "store" ? null : catalog.listings.find(l => l.hash === target) ?? null;
  const [rating, setRating] = useState<Rating>(asRating(catalog.store.contentRating));
  const [tags, setTags] = useState<string[]>([]);
  const [status, setStatus] = useState("");

  function choose(next: string) {
    setTarget(next);
    setStatus("");
    const listing = next === "store" ? null : catalog.listings.find(l => l.hash === next);
    setRating(asRating(listing ? listing.contentRating : catalog.store.contentRating));
    setTags(listing ? listing.contentTags : []);
  }

  async function save() {
    setStatus("Signing…");
    try {
      const result = await setContentLabel({ target, rating, tags: current ? tags : [] });
      onCatalog(result.catalog);
      if (result.hash) setTarget(result.hash);
      setStatus(result.hash ? `Signed. The listing's new address is ${result.hash.slice(0, 12)}…` : "Signed.");
    } catch (e) {
      setStatus(e instanceof Error ? e.message : "The label could not be saved.");
    }
  }

  return <section>
    <h2>Your store's labels</h2>
    <p className="ob-page-intro">Rate what you sell. Buyers' devices use this to decide what to show. Relabeling a listing signs it again, which gives it a new address.</p>
    <label className="ob-field"><span>Label</span><select value={target} onChange={e => choose(e.target.value)}>
      <option value="store">The whole store</option>
      {catalog.listings.map(l => <option key={l.hash} value={l.hash}>{l.title}</option>)}
    </select></label>
    <label className="ob-field"><span>Rating</span><select value={rating} onChange={e => setRating(e.target.value as Rating)}>
      {RATINGS.map(r => <option key={r} value={r}>{r}</option>)}
    </select></label>
    {current ? <fieldset><legend>Tags</legend>{CONTENT_TAGS.map(tag => <label key={tag}><input type="checkbox" checked={tags.includes(tag)} onChange={() => setTags(t => t.includes(tag) ? t.filter(x => x !== tag) : [...t, tag])}/> {tag} </label>)}</fieldset> : null}
    <button className="ob-primary" onClick={() => void save()}>Save label</button>
    {status ? <p className="ob-page-intro" role="status">{status}</p> : null}
  </section>;
}

function asRating(value: ContentRating): Rating {
  return value === "unrated" ? "general" : value;
}
