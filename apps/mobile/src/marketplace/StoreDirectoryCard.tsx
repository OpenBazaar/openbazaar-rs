import type { StoreSummary } from "../records";
import { formatRating, listingCountLabel, monogram, railLabels } from "../storeDirectory";
import { FollowButton, type SharedDirectory } from "./FollowButton";
import { MediaImage } from "./SellerKit";

const MAX_CATEGORY_PILLS = 3;

/** One store in the directory. The whole card opens the store; Follow sits above that and does not. */
export function StoreDirectoryCard({ store, onOpen, onFollowChanged, directory }: { store: StoreSummary; onOpen: (storeId: string) => void; onFollowChanged: (message: string) => void; directory: SharedDirectory }) {
  const meta = [store.region, listingCountLabel(store.listingCount)].filter(Boolean);
  const hasRating = store.reviewCount > 0 && store.reviewAverage !== null;
  return <article className="ob-directory-card">
    <div className="ob-directory-banner" aria-hidden="true">{store.banner && <MediaImage hash={store.banner} alt="" className="ob-directory-banner-img"/>}</div>
    <div className="ob-directory-body">
      <div className="ob-directory-head">
        <span className="ob-directory-avatar" aria-hidden="true">{store.avatar ? <MediaImage hash={store.avatar} alt="" fallback={monogram(store.name)}/> : monogram(store.name)}</span>
        <div className="ob-directory-title">
          <h2><button type="button" className="ob-directory-open" aria-label={`Open store ${store.name}`} onClick={() => onOpen(store.key)}>{store.name}</button></h2>
          {store.own && <span className="ob-directory-own">Your store</span>}
        </div>
        {!store.own && <div className="ob-directory-follow" onClick={event => event.stopPropagation()}><FollowButton storeId={store.key} onChanged={onFollowChanged} shared={directory}/></div>}
      </div>
      {store.tagline && <p className="ob-directory-tagline">{store.tagline}</p>}
      <p className="ob-directory-meta">
        {meta.map(part => <span key={part}>{part}</span>)}
        {hasRating && <span className="ob-directory-rating" role="img" aria-label={`Rated ${formatRating(store.reviewAverage ?? 0)} out of 5 from ${store.reviewCount} review${store.reviewCount === 1 ? "" : "s"}`}>★ {formatRating(store.reviewAverage ?? 0)} <span>({store.reviewCount.toLocaleString("en-US")})</span></span>}
      </p>
      <ul className="ob-directory-pills" aria-label="Payment methods and categories">
        {railLabels(store).map(rail => <li key={rail} className="ob-directory-pill is-rail">{rail}</li>)}
        {store.categories.slice(0, MAX_CATEGORY_PILLS).map(name => <li key={name} className="ob-directory-pill">{name}</li>)}
      </ul>
    </div>
  </article>;
}
