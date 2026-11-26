import { useEffect, useMemo, useState } from "react";
import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import { listPage } from "../listPage";
import type { StoreSummary } from "../records";
import type { Verdict } from "../safety/filter";
import { storefrontImages } from "../stores";
import { Empty, Icon, ProductCard } from "./Marketplace";
import { CoveredCard } from "./Safety";
import { MediaImage } from "./SellerKit";
import { FollowButton } from "./FollowButton";

/** A store this node already holds: its signed record and the catalog listings for that key. */
export function CatalogStore({ storeId, followable, onFollowChanged, store, listings, rates, currency, revealed, onReveal, verdictOf, onOpen, onStore, onBack }: {
  storeId: string;
  /** False for this node's own store. */
  followable: boolean;
  onFollowChanged: (message: string) => void;
  store: StoreSummary | null;
  listings: ListingHeader[];
  rates: RateTable;
  currency: CurrencyCode;
  revealed: readonly string[];
  onReveal: (hash: string) => void;
  verdictOf: (listing: ListingHeader) => Verdict;
  onOpen: (listing: ListingHeader) => void;
  onStore: (storeId: string) => void;
  onBack: () => void;
}) {
  const details = store?.record.details ?? null;
  const images = storefrontImages({ avatar: store?.avatar, details });
  const name = store?.name || listings[0]?.seller || "Store";
  const region = store?.region ?? "";
  const tagline = store?.tagline ?? "";
  const about = details?.about ?? "";
  const policies = [["Processing", details?.processingTime ?? ""], ["Shipping", details?.shippingPolicy ?? ""], ["Returns", details?.returnsPolicy ?? ""]].filter(([, text]) => text);
  const meta = [region, details?.shipsTo.length ? `Ships to ${details.shipsTo.join(", ")}` : "", `${listings.length} listing${listings.length === 1 ? "" : "s"}`].filter(Boolean).join(" · ");
  const ordered = useMemo(
    () => [...listings].sort((a, b) => b.listedOn - a.listedOn || a.title.localeCompare(b.title)),
    [listings],
  );
  const [pageNum, setPageNum] = useState(0);
  const [pageSize, setPageSize] = useState(24);
  const paged = useMemo(() => listPage(ordered, pageNum, pageSize), [ordered, pageNum, pageSize]);
  useEffect(() => { setPageNum(0); }, [storeId, pageSize, listings]);

  function changePage(next: number) {
    setPageNum(next);
  }

  function pagination(bottom = false) {
    return <nav className="ob-order-pagination" aria-label={`Store listings pagination${bottom ? " bottom" : ""}`}>
      <button type="button" aria-label="First page" disabled={paged.page === 0} onClick={() => changePage(0)}>«</button>
      <button type="button" disabled={paged.page === 0} onClick={() => changePage(paged.page - 1)}>Previous</button>
      <span>Page {paged.page + 1} of {paged.pages.toLocaleString()}</span>
      <button type="button" disabled={paged.page + 1 >= paged.pages} onClick={() => changePage(paged.page + 1)}>Next</button>
      <button type="button" aria-label="Last page" disabled={paged.page + 1 >= paged.pages} onClick={() => changePage(paged.pages - 1)}>»</button>
    </nav>;
  }

  return <section className="ob-storefront" aria-labelledby="ob-store-page-name">
    <button className="ob-back" onClick={onBack}><Icon name="arrow" size={16}/>Back to products</button>
    <div className="ob-storefront-banner">{images.banner && <MediaImage hash={images.banner} alt="" className="ob-store-banner-img"/>}</div>
    <header className="ob-storefront-head">
      <span className="ob-store-avatar is-large">{images.avatar ? <MediaImage hash={images.avatar} alt={`${name} avatar`}/> : name.charAt(0)}</span>
      <div>
        <h1 id="ob-store-page-name">{name}</h1>
        {tagline && <p className="ob-storefront-tagline">{tagline}</p>}
        <p className="ob-meta">{meta}</p>
      </div>
      {followable && <FollowButton storeId={storeId} onChanged={onFollowChanged}/>}
    </header>
    <div className="ob-storefront-body">
      <div>
        {ordered.length ? <>
          <div className="ob-order-list-meta">
            <p role="status" aria-live="polite">
              {ordered.length > pageSize
                ? `${(paged.start + 1).toLocaleString()}–${Math.min(paged.start + pageSize, ordered.length).toLocaleString()} of ${ordered.length.toLocaleString()} listings`
                : `${ordered.length.toLocaleString()} listing${ordered.length === 1 ? "" : "s"}`}
            </p>
            <label className="ob-sort">Per page <select aria-label="Listings per page" value={pageSize} onChange={e => setPageSize(Number(e.target.value))}><option value={24}>24</option><option value={48}>48</option></select></label>
            {ordered.length > pageSize ? pagination() : null}
          </div>
          <div className="ob-product-grid">{paged.items.map(listing => {
            const verdict = verdictOf(listing);
            return verdict.action === "cover" && !revealed.includes(listing.hash)
              ? <CoveredCard key={listing.hash} reason={verdict.reason} onShow={() => onReveal(listing.hash)}/>
              : <ProductCard key={listing.hash} listing={listing} rates={rates} currency={currency} saved={false} onOpen={() => onOpen(listing)} onStore={() => onStore(listing.storeId)}/>;
          })}</div>
          {ordered.length > pageSize ? <div className="ob-order-pagination-footer">{pagination(true)}</div> : null}
        </> : <Empty icon="bag" title="No listings" text="This store has no goods in the catalog."/>}
      </div>
      {(about || policies.length > 0) && <aside className="ob-storefront-about">
        {about && <><h2>About</h2><p>{about}</p></>}
        {policies.map(([title, text]) => <div key={title}><h3>{title}</h3><p>{text}</p></div>)}
      </aside>}
    </div>
  </section>;
}
