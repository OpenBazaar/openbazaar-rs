import type { CurrencyCode, ListingHeader, RateTable } from "../catalog";
import type { SellerProfile } from "../seller";
import { storefrontImages } from "../stores";
import { Empty, ProductCard } from "./Marketplace";
import { MediaImage } from "./SellerKit";
import { RailMark } from "./RailMark";
import { StoreReviewsPanel } from "./Reviews";
import { StoreAddress } from "./StoreAddress";

/** The seller's store as buyers see it: the signed storefront and its listings. */
export function OwnStore({ profile, listings, rates, currency, onOpen, onEdit, onAddListing }: { profile: SellerProfile; listings: ListingHeader[]; rates: RateTable; currency: CurrencyCode; onOpen: (listing: ListingHeader) => void; onEdit: () => void; onAddListing: () => void }) {
  const own = listings.filter(listing => listing.storeId === profile.storeKey);
  const policies = [["Processing", profile.processingTime], ["Shipping", profile.shippingPolicy], ["Returns", profile.returnsPolicy]].filter(([, text]) => text);
  const images = storefrontImages(profile);
  return <section className="ob-storefront" aria-labelledby="ob-storefront-name">
    <div className="ob-storefront-banner">{images.banner && <MediaImage hash={images.banner} alt="" className="ob-store-banner-img"/>}</div>
    <header className="ob-storefront-head">
      <span className="ob-store-avatar is-large">{images.avatar ? <MediaImage hash={images.avatar} alt={`${profile.name} avatar`}/> : profile.name.charAt(0)}</span>
      <div>
        <h1 id="ob-storefront-name">{profile.name}</h1>
        {profile.tagline && <p className="ob-storefront-tagline">{profile.tagline}</p>}
        <p className="ob-meta">{[profile.region, profile.shipsTo.length ? `Ships to ${profile.shipsTo.join(", ")}` : "", `${own.length} listing${own.length === 1 ? "" : "s"}`].filter(Boolean).join(" · ")}</p>
      </div>
      <div className="ob-storefront-actions"><button className="ob-secondary" onClick={onEdit}>Edit store</button><button className="ob-primary" onClick={onAddListing}>New listing</button></div>
    </header>
    {profile.unpublishedChanges && <p className="ob-form-error" role="status">Some changes wait for your wallet to be unlocked before your node can sign them.</p>}
    <div className="ob-storefront-body">
      <div>
        {own.length ? <div className="ob-product-grid">{own.map(listing => <ProductCard key={listing.hash} listing={listing} rates={rates} currency={currency} saved={false} onOpen={() => onOpen(listing)}/>)}</div> : <Empty icon="bag" title="No listings yet" text="Buyers see your listings here once you add them."/>}
      </div>
      <aside className="ob-storefront-about">
        {profile.about && <><h2>About</h2><p>{profile.about}</p></>}
        {policies.map(([title, text]) => <div key={title}><h3>{title}</h3><p>{text}</p></div>)}
        <h3>Accepts</h3>
        <p className="ob-rail"><RailMark rail="bitcoin" />Bitcoin{profile.network === "testnet4" ? " (testnet4)" : ""}, paid directly</p>
        {profile.acceptsXmr && <p className="ob-rail"><RailMark rail="monero" />Monero, paid directly</p>}
        {profile.acceptsLightning && <p className="ob-rail"><RailMark rail="lightning" />Lightning, paid directly</p>}
        <StoreAddress/>
        <StoreReviewsPanel storeKey={profile.storeKey} own/>
      </aside>
    </div>
  </section>;
}
