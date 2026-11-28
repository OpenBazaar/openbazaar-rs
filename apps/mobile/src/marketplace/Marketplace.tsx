import { WalletPage } from "./WalletPage";
import { useContext, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { afterShopPair, fetchCatalog, fetchLabelers, fetchRates, hasNodeHost, NodeOfflineError, type LabelerState, type BuyerOrder, type CurrencyCode, type ListingHeader, type RateTable, type ShopCatalog } from "../catalog";
import { clearListingImages, fetchHostStore, fetchPublicCatalog, NOT_A_STORE, PAYMENT_NOTE, setDirectBrowse, setListingImages, STORE_KEY, storeBase, storeFromLocation } from "../directStore";
import { loadPublicMarket } from "../publicMarket";
import { servedByHomeNode } from "../homeNode/transport";
import { listPage } from "../listPage";
import { conditionChoices, filterListings, matchingStores, narrowingActive, setSpecificChoice, specificChoices } from "../search";
import { CATEGORY_NAMES, categoryLabel, topCategory } from "../listingFields";
import { CONDITION_LABELS } from "../commerce";
import { fetchStore } from "../stores";
import { Bag, Addresses, MarketplaceCheckout, CheckoutConfirmation } from "./Commerce";
import { lineKey, type CartLine } from "../cart";
import { ListingBuyFacts, ListingBuyTerms, QuantityStepper, useBuyChoice, type BuyChoice } from "./ListingBuy";
import { OfferSheet } from "./OfferSheet";
import { ListingReviews } from "./Reviews";
import { type CheckoutReceipt } from "../catalog";
import { safetyVerdict, visibleListings } from "../safety/filter";
import { readSafety, writeSafety } from "../safety/storage";
import { CoveredCard, HideActions, SafetyInterstitial, SafetySettingsPanel } from "./Safety";
import { VendorLabelPicker } from "./VendorLabelPicker";
import { LabelersPanel } from "./Labelers";
import { MarketEmptyArtwork } from "./MarketArtwork";
import { NetworkBadge, NodePaymentsProvider, OrderPayment } from "./NodePayments";
import { useNodeResource } from "../useNodeResource";
import { PageHeading } from "./PageHeading";
import { BuildMismatchWarning, useBuildInfo } from "./BuildInfo";
import { HomeNodePanel } from "./HomeNodePanel";
import { HomeNodeBadge, ModeratorDeskLink, OpenSignIn } from "./HomeNodeGate";
import { LightningSettings } from "./LightningSettings";
import { ZcashSettings } from "./ZcashSettings";
import { MoneroSettings } from "./MoneroSettings";
import { TorSettings } from "./TorStatus";
import { UpdateBanner } from "./UpdateBanner";
import { UpdateSettings } from "./UpdateSettings";
import { useDesktopUpdater } from "./useDesktopUpdater";
import { fetchNetwork } from "../nodeWallet";
import { LightningOrderPayment } from "./LightningPayment";
import { priceDrops, rememberPrices, savedHashesAfterRefresh, sameWatches, signedPrice, type WatchedListing } from "../priceNotices";
import { fetchProfile, formatPrice, setRole } from "../seller";
import { readSalesSeen, salesBadge, salesSeenSnapshot, writeSalesSeen, type SaleSight, type SalesSeen } from "../salesBadge";
import { Onboarding } from "./Onboarding";
import { StoreSetup } from "./StoreSetup";
import { Coupons } from "./Coupons";
import { StoreListings } from "./StoreListings";
import { Sales, useSales } from "./Sales";
import { Orders } from "./Orders";
import { OwnStore } from "./OwnStore";
import { CatalogStore } from "./CatalogStore";
import { shopHistory, type ShopPage } from "./shopHistory";
import { Follows } from "./Follows";
import { admittedStoreIds, fetchDirectory } from "../follows";
import { FollowButton } from "./FollowButton";
import { ListingRails } from "./RailMark";
import { MediaImage } from "./SellerKit";
import { PhotoZoom } from "./PhotoZoom";
import { PairingBanner, PairingDialog } from "./PairingDialog";
import { DirectBanner, DirectPay, OpenStoreForm, SellGate } from "./DirectBrowse";
import { TorOrders, type KnownVendorStore } from "./TorOrders";
import { parseReceiptHash, type ParsedReceiptHash } from "./torReceiptLink";
import { createWebBuyerStore, subscribeWebBuyerOrders } from "../buyerLight/webStore";
import { BrowsersSettings } from "./BrowsersSettings";
import { CatalogEmpty } from "./CatalogEmpty";
import { sanitizeGuestLocation } from "./guestRoute";
import { SidebarUtilities } from "./SidebarUtilities";
import { Classifieds } from "./Classifieds";
import { StoreDirectory } from "./StoreDirectory";
import { StoreDirectoryCard } from "./StoreDirectoryCard";
import { DEFAULT_OPTIONS, directoryCategories, filterAndSortStores, filtersActive, type DirectoryRail, type DirectorySort } from "../storeDirectory";
import { combineSalesBadge, useOrderMessageAlerts } from "./useOrderMessageAlerts";

type IconName = "settings" | "wallet" | "sun" | "moon" | "compass" | "heart" | "bag" | "arrow" | "search" | "chevron" | "check" | "shield" | "globe" | "close" | "menu" | "book" | "cup" | "home" | "music" | "shirt" | "tool" | "paper" | "grid" | "pin" | "bolt" | "external" | "ticket" | "store";
export function Icon({ name, size = 20, ...props }: { name: IconName; size?: number; className?: string }) {
  const paths: Record<IconName, ReactNode> = {
    settings: <><path d="m9 3-1 3-3 1-2 5 2 5 3 1 1 3h6l1-3 3-1 2-5-2-5-3-1-1-3Z"/><circle cx="12" cy="12" r="3"/></>,
    wallet: <><rect x="3" y="6" width="18" height="14" rx="2"/><path d="M3 8V5a2 2 0 0 1 2-2h12v3M21 11h-6v5h6"/></>,
    sun: <><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5"/></>,
    moon: <path d="M20.5 13A8.5 8.5 0 0 1 11 3.5 8.5 8.5 0 1 0 20.5 13Z"/>,
    compass: <><circle cx="12" cy="12" r="9"/><path d="m16 8-2.5 5.5L8 16l2.5-5.5Z"/></>,
    heart: <path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z"/>,
    bag: <><path d="M5 7h14l1 14H4L5 7Z"/><path d="M9 8V6a3 3 0 0 1 6 0v2"/></>,
    arrow: <><path d="M4 12h16m-6-6 6 6-6 6"/></>,
    search: <><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/></>,
    chevron: <path d="m9 5 7 7-7 7"/>,
    check: <path d="m5 12 4 4L19 6"/>,
    shield: <><path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z"/><path d="m8 11 3 3 5-5"/></>,
    globe: <><circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/></>,
    close: <path d="m6 6 12 12M6 18 18 6"/>,
    menu: <path d="M4 6h16M4 12h16M4 18h16"/>,
    book: <><path d="M4 4h6l2 2 2-2h6v15h-6l-2 2-2-2H4Z"/><path d="M12 6v15"/></>,
    cup: <><path d="M5 8h12v9a3 3 0 0 1-3 3H8a3 3 0 0 1-3-3V8ZM17 9h2a3 3 0 0 1 0 6h-2M8 3v2m5-2v2"/></>,
    home: <><path d="m3 10 9-7 9 7M5 9v12h14V9M9 21v-8h6v8"/></>,
    music: <><path d="M9 18V5l11-2v13M9 8l11-2"/><ellipse cx="6" cy="18" rx="3" ry="2.5"/><ellipse cx="17" cy="16" rx="3" ry="2.5"/></>,
    shirt: <path d="m8 3-5 4 3 5 2-1v10h8V11l2 1 3-5-5-4a4 4 0 0 1-8 0Z"/>,
    tool: <><path d="m14 6 4 4m-8 0-7 8 3 3 8-7M14 3l-4 4 7 7 4-4M13 4l7 7"/></>,
    paper: <><path d="M5 3h10l4 4v14H5V3Zm10 0v5h4M8 12h8m-8 4h5"/></>,
    grid: <><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></>,
    pin: <><path d="M19 10c0 5-7 11-7 11S5 15 5 10a7 7 0 0 1 14 0Z"/><circle cx="12" cy="10" r="2"/></>,
    bolt: <path d="m13 2-9 12h7l-1 8 10-13h-8l1-7Z"/>,
    external: <path d="M14 3h7v7m0-7L10 14M10 3H3v18h18v-7"/>,
    store: <><path d="m4 9 1.5-5h13L20 9"/><path d="M4 9a2.67 2.67 0 0 0 5.33 0 2.67 2.67 0 0 0 5.34 0A2.67 2.67 0 0 0 20 9"/><path d="M5 12v8h14v-8M10 20v-5h4v5"/></>,
    ticket: <><path d="M4 8a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v2a2 2 0 0 0 0 4v2a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-2a2 2 0 0 0 0-4Z"/><path d="M9 6v12"/></>,
  };
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...props}>{paths[name]}</svg>;
}
const categoryIcons: ReadonlyMap<string, IconName> = new Map<string, IconName>([["Books", "book"], ["Clothing and shoes", "shirt"], ["Electronics", "bolt"], ["Home and kitchen", "home"], ["Music and media", "music"], ["Tools and supplies", "tool"], ["Food and drink", "cup"], ["Digital goods", "external"]]);
const photos: Record<string, string> = { "Paperback novel": "paperback", "Field guide to tide pools": "paperback-open", "Whole bean coffee, 12 oz": "coffee", "Wildflower honey, 8 oz": "honey", "Linen dish towel": "towel", "Stoneware mug": "mug", "Wool socks": "socks", "Clothbound notebook": "notebook", Sketchbook: "notebook", "Beeswax candles, set of 4": "candles", "Cotton work shirt": "shirt", "Work shirt": "shirt", "Cotton apron": "shirt", "Garden trowel": "trowel", "Letterpress cards": "cards" };
function photo(listing: ListingHeader) { return Object.hasOwn(photos, listing.title) ? `goods/${photos[listing.title]}.jpg` : null; }
function readSaved(): string[] { try { const a = JSON.parse(localStorage.getItem("ob-shop-saved") ?? "[]"); return Array.isArray(a) ? a.filter((s): s is string => typeof s === "string") : []; } catch { return []; } }
function readWatched(): WatchedListing[] {
  try {
    const parsed = JSON.parse(localStorage.getItem("ob-price-watched") ?? "[]");
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is WatchedListing => !!item && typeof item.listingId === "string" && typeof item.storeId === "string" && typeof item.hash === "string" && Number.isSafeInteger(item.amount) && typeof item.currency === "string");
  } catch {
    return [];
  }
}
function readCurrency(): CurrencyCode { try { const c = localStorage.getItem("ob-shop-currency"); return ["USD", "EUR", "GBP", "JPY"].includes(c ?? "") ? c as CurrencyCode : "USD"; } catch { return "USD"; } }
function readSidebarWidth() { try { const n = Number(localStorage.getItem("ob-sidebar-width")); return n >= 224 && n <= 360 ? n : 244; } catch { return 244; } }
function readCollapsed() { try { return localStorage.getItem("ob-sidebar-collapsed") === "true"; } catch { return false; } }
function fiat(sats: number, rates: RateTable, currency: CurrencyCode) { const r = rates[currency]; return r ? new Intl.NumberFormat("en-US", { style: "currency", currency }).format(sats / 1e8 * r) : null; }
function btc(sats: number) { return `${(sats / 1e8).toFixed(8).replace(/0+$/, "").replace(/\.$/, "")} BTC`; }
export function Money({ sats, rates, currency, large = false }: { sats: number; rates: RateTable; currency: CurrencyCode; large?: boolean }) { const estimate = fiat(sats, rates, currency); return <div className={`ob-money ${large ? "ob-money-large" : ""}`}><span>{sats.toLocaleString("en-US")} <small>sats</small></span>{estimate && <span className="ob-estimate" title={`Estimated value in ${currency}`}>≈ {estimate}</span>}</div>; }
function Brand({ theme }: { theme: "light" | "dark" }) { return <span className="ob-brand"><span className="ob-brand-icon"><img src={theme === "light" ? "brand/ob-market-logo-light.png" : "brand/ob-market-logo-256.png"} alt="" width={42} height={42}/></span><span>OpenBazaar<span className="ob-brand-dot">.</span></span></span>; }

export type Section = "discover" | "saved" | "orders" | "bag" | "addresses" | "safety" | "wallet" | "store-labels" | "home-node" | "browsers" | "lightning" | "zcash" | "monero" | "tor" | "updates" | "follows" | "classifieds" | "stores" | StoreSection;
type StoreSection = "storefront" | "listings" | "coupons" | "sales" | "store";
const STORE_SECTIONS: readonly Section[] = ["storefront", "listings", "coupons", "sales", "store"];
type ToastNotice = string | { message: string; action?: { label: string; onClick: () => void } };
function emptyFilters() { return { seller: "", kind: "", shipFrom: "", min: "", max: "", condition: "", specifics: {} as Record<string, string> }; }

const GUEST_RAILS: [DirectoryRail, string][] = [["all", "All rails"], ["btc", "Bitcoin"], ["ln", "Lightning"], ["xmr", "Monero"], ["zec", "Zcash"]];
const GUEST_SORTS: [DirectorySort, string][] = [["listings", "Most listings"], ["rating", "Top rated"], ["name", "Name A–Z"]];

/** Read-only store directory for guest/public mode, powered by catalog.stores. */
function GuestStoreDirectory({ catalog, onOpen }: { catalog: ShopCatalog | null; onOpen: (storeId: string) => void }) {
  const [filters, setFilters] = useState(DEFAULT_OPTIONS);
  const all = useMemo(() => catalog?.stores ?? [], [catalog]);
  const emptyFollowed = useMemo(() => new Set<string>(), []);
  const visible = useMemo(() => filterAndSortStores(all, { ...filters, scope: "all", followedIds: emptyFollowed }), [all, filters, emptyFollowed]);
  const categories = useMemo(() => directoryCategories(all), [all]);
  const set = (change: Partial<typeof DEFAULT_OPTIONS>) => setFilters(current => ({ ...current, ...change }));
  const plural = (n: number) => `${n.toLocaleString("en-US")} store${n === 1 ? "" : "s"}`;

  return <section className="ob-directory" aria-labelledby="ob-directory-title">
    <header className="ob-directory-header">
      <div>
        <h1 id="ob-directory-title">Stores<span aria-label={plural(visible.length)}>{visible.length}</span></h1>
        <p>Browse stores on the network. Open one to see its goods.</p>
      </div>
    </header>
    <div className="ob-directory-controls">
      <label className="ob-search ob-directory-search">
        <Icon name="search" size={18}/>
        <input aria-label="Search stores" placeholder="Search stores by name, region, or category…" value={filters.query} onChange={event => set({ query: event.target.value })}/>
        {filters.query && <button type="button" className="ob-icon-button" aria-label="Clear search" onClick={() => set({ query: "" })}><Icon name="close" size={16}/></button>}
      </label>
      <div className="ob-directory-selects">
        <label className="ob-directory-select"><span className="ob-sr-only">Category</span>
          <select aria-label="Category" value={filters.category} onChange={event => set({ category: event.target.value })}>
            <option value="">All categories</option>
            {categories.map(category => <option key={category.name} value={category.name}>{category.name} ({category.count})</option>)}
          </select>
        </label>
        <label className="ob-directory-select"><span className="ob-sr-only">Payment rail</span>
          <select aria-label="Payment rail" value={filters.rail} onChange={event => set({ rail: event.target.value as DirectoryRail })}>
            {GUEST_RAILS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        <label className="ob-directory-select"><span className="ob-sr-only">Sort stores</span>
          <select aria-label="Sort stores" value={filters.sort} onChange={event => set({ sort: event.target.value as DirectorySort })}>
            {GUEST_SORTS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        {filtersActive({ ...filters, scope: "all" }) && <button type="button" className="ob-text-link" onClick={() => setFilters(current => ({ ...DEFAULT_OPTIONS, sort: current.sort }))}>Clear filters</button>}
      </div>
    </div>
    {visible.length ? <>
      <p className="ob-directory-status" role="status" aria-live="polite">{visible.length === all.length ? plural(all.length) : `${visible.length.toLocaleString("en-US")} of ${plural(all.length)}`}</p>
      <div className="ob-directory-grid">{visible.map(store => <article key={store.key} className="ob-directory-card"><div className="ob-directory-banner" aria-hidden="true">{store.banner && <MediaImage hash={store.banner} alt="" className="ob-directory-banner-img"/>}</div><div className="ob-directory-body"><div className="ob-directory-head"><span className="ob-directory-avatar" aria-hidden="true">{store.avatar ? <MediaImage hash={store.avatar} alt="" fallback={store.name.charAt(0).toUpperCase()}/> : store.name.charAt(0).toUpperCase()}</span><div className="ob-directory-title"><h2><button type="button" className="ob-directory-open" aria-label={`Open store ${store.name}`} onClick={() => onOpen(store.key)}>{store.name}</button></h2></div></div>{store.tagline && <p className="ob-directory-tagline">{store.tagline}</p>}<p className="ob-directory-meta">{store.region && <span>{store.region}</span>}<span>{store.listingCount} {store.listingCount === 1 ? "item" : "items"}</span></p></div></article>)}</div>
    </> : <Empty icon="store" title="No stores yet" text="No stores are published on this node right now."/>}
  </section>;
}

export default function Marketplace() {
  const [catalog, setCatalog] = useState<ShopCatalog | null>(null);
  const [loadError, setLoadError] = useState("");
  const [loading, setLoading] = useState(true);
  const [offline, setOffline] = useState(false);
  const [direct, setDirect] = useState<string | null>(null);
  const [publicMode, setPublicMode] = useState(false);
  const [onionNote, setOnionNote] = useState<string | null>(null);
  const [storeAddresses, setStoreAddresses] = useState<Map<string, string>>(new Map());
  const openSignIn = useContext(OpenSignIn);
  const [storeDraft, setStoreDraft] = useState("");
  const [rates, setRates] = useState<RateTable>({});
  const [currency, setCurrency] = useState<CurrencyCode>(readCurrency);
  const [section, setSection] = useState<Section>("discover");
  const [page, setPage] = useState<ShopPage>({ name: "browse" });
  const [category, setCategory] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState("newest");
  const [listPageNum, setListPageNum] = useState(0);
  const [pageSize, setPageSize] = useState(24);
  const [filters, setFilters] = useState(emptyFilters);
  const [saved, setSaved] = useState<string[]>(readSaved);
  const [watched, setWatched] = useState<WatchedListing[]>(readWatched);
  const [cart, setCart] = useState<CartLine[]>(() => { try { const a = JSON.parse(localStorage.getItem("ob-shop-cart") ?? "[]"); return Array.isArray(a) ? a.filter(l => typeof l.hash === "string" && Number.isInteger(l.quantity) && l.quantity > 0 && l.quantity <= 99) : []; } catch { return []; } });
  const [menu, setMenu] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [theme, setTheme] = useState<"light" | "dark">(() => { try { const t = localStorage.getItem("ob-theme"); if (t === "light" || t === "dark") return t; } catch {} return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"; });
  useEffect(() => { document.documentElement.dataset.obTheme = theme; try { localStorage.setItem("ob-theme", theme); } catch {} }, [theme]);
  const [toast, setToast] = useState<ToastNotice>("");
  const toastMessage = typeof toast === "string" ? toast : toast.message;
  const toastAction = typeof toast === "string" ? undefined : toast.action;
  const guest = !!direct || publicMode || !hasNodeHost();
  const nodeAvailable = !guest;
  const [guestOrderCount, setGuestOrderCount] = useState(0);
  const [pendingReceiptRestore, setPendingReceiptRestore] = useState<ParsedReceiptHash | null>(null);
  useEffect(() => {
    if (!guest) return;
    const webStore = createWebBuyerStore();
    const sync = () => { void webStore.loadOrders().then((list) => setGuestOrderCount(list.length)); };
    sync();
    return subscribeWebBuyerOrders(sync);
  }, [guest]);
  const knownVendorStores = useMemo<KnownVendorStore[]>(() => {
    const out: KnownVendorStore[] = [];
    const seen = new Set<string>();
    const add = (name: string, url: string) => {
      const clean = url.trim().replace(/\/+$/, "");
      if (!clean || seen.has(clean)) return;
      seen.add(clean);
      out.push({ name: name || clean, url: clean });
    };
    if (direct) {
      add(catalog?.store.name ?? direct, direct);
    }
    for (const [storeId, url] of storeAddresses.entries()) {
      const match = catalog?.stores.find((s) => s.key === storeId);
      add(match?.name ?? url, url);
    }
    return out;
  }, [direct, storeAddresses, catalog]);
  const build = useBuildInfo(nodeAvailable);
  const updater = useDesktopUpdater();
  const profile = useNodeResource(fetchProfile, 0, nodeAvailable);
  const directory = useNodeResource(fetchDirectory, 15000, nodeAvailable);
  const [openedShop, setOpenedShop] = useState<{ id: string; listings: ListingHeader[] } | null>(null);
  const network = useNodeResource(fetchNetwork, 0, nodeAvailable);
  const openDesk = useContext(ModeratorDeskLink);
  const seller = profile.data?.role === "seller";
  const sales = useSales(seller, sale => setToast(`New order: ${sale.lines.map(l => l.title).join(", ")}`), section === "sales");
  const storeKey = profile.data?.storeKey ?? "";
  const activeOrderId = page.name === "order" ? page.orderId : null;
  const activeSaleId = page.name === "sale" ? page.saleId : null;
  const messageAlerts = useOrderMessageAlerts({
    nodeAvailable,
    seller,
    storeKey,
    viewingOrders: section === "orders",
    viewingSales: section === "sales",
    activeOrderId,
    activeSaleId,
    salesData: sales.data,
    onAlert: setToast,
    onNavigate: (side, orderId) => {
      setToast("");
      if (orderId) {
        if (side === "orders") openOrder(orderId);
        else openSale(orderId);
      } else {
        navigate(side);
      }
    },
  });
  useEffect(() => {
    if (activeOrderId) messageAlerts.markOrderSeen("orders", activeOrderId);
    else if (activeSaleId) messageAlerts.markOrderSeen("sales", activeSaleId);
  }, [activeOrderId, activeSaleId, messageAlerts.markOrderSeen]);
  const [salesSeen, setSalesSeen] = useState<SalesSeen | null>(null);
  useEffect(() => { setSalesSeen(readSalesSeen(storeKey)); }, [storeKey]);
  const salesCount = combineSalesBadge(salesBadge(sales.data ?? [], salesSeen), messageAlerts.unreadSales);
  function rememberSales(list: SaleSight[]) {
    if (!storeKey) return;
    const next = salesSeenSnapshot(list);
    setSalesSeen(next);
    writeSalesSeen(storeKey, next);
  }
  const [newListing, setNewListing] = useState(0);
  const [safety, setSafety] = useState(readSafety);
  const [revealed, setRevealed] = useState<string[]>([]);
  const [labelers, setLabelerState] = useState<LabelerState | null>(null);
  const [sidebarWidth, setSidebarWidth] = useState(readSidebarWidth);
  const [collapsed, setCollapsed] = useState(readCollapsed);
  const [mobile, setMobile] = useState(() => window.innerWidth < 960);
  const [resizing, setResizing] = useState(false);
  const sidebarRef = useRef<HTMLElement>(null);
  const content = useRef<HTMLElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const loadSeq = useRef(0);
  const focusSearch = useRef(false);
  useEffect(() => { if (focusSearch.current && searchRef.current) { searchRef.current.focus(); focusSearch.current = false; } }, [section, page.name]);
  const settingsToggleRef = useRef<HTMLButtonElement>(null);
  const settingsMenuRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!settingsOpen) return;
    const dismiss = (event: PointerEvent) => { if (!settingsToggleRef.current?.contains(event.target as Node) && !settingsMenuRef.current?.contains(event.target as Node)) setSettingsOpen(false); };
    document.addEventListener("pointerdown", dismiss);
    return () => document.removeEventListener("pointerdown", dismiss);
  }, [settingsOpen]);
  async function load(pasted?: string) {
    const seq = ++loadSeq.current;
    const current = () => seq === loadSeq.current;
    setLoading(true);
    setLoadError("");
    try {
      const next = await fetchCatalog();
      if (!current()) return;
      clearListingImages();
      setDirectBrowse(null);
      setDirect(null);
      setPublicMode(false);
      setOnionNote(null);
      setOffline(false);
      setCatalog(next);
    } catch (e) {
      if (!current()) return;
      const down = e instanceof NodeOfflineError;
      setOffline(down);
      if (!down) {
        setDirectBrowse(null);
        setDirect(null);
        setLoadError(e instanceof Error ? e.message : "Your node could not be reached.");
        return;
      }
      const choice = pasted !== undefined ? { url: storeBase(pasted), rejected: storeBase(pasted) === null } : storeFromLocation(window.location, servedByHomeNode());
      if (!choice.url) {
        if (choice.rejected || pasted !== undefined) {
          clearListingImages();
          setDirectBrowse(null);
          setDirect(null);
          setPublicMode(false);
          setOnionNote(null);
          setCatalog(null);
          setLoadError(NOT_A_STORE);
          return;
        }
        try {
          const hostStore = await fetchHostStore(window.location, servedByHomeNode());
          if (!current()) return;
          if (hostStore) {
            clearListingImages();
            setDirectBrowse(hostStore.url);
            setDirect(hostStore.url);
            setPublicMode(false);
            setOnionNote(null);
            setCatalog(hostStore.catalog);
            setLoadError("");
            return;
          }
          const market = await loadPublicMarket(window.location);
          if (!current()) return;
          clearListingImages();
          for (const image of market.images) setListingImages(image.url, image.hashes);
          setDirectBrowse(null);
          setDirect(null);
          setPublicMode(true);
          setOnionNote(market.notice);
          setStoreAddresses(market.storeAddresses);
          setCatalog(market.catalog);
          setLoadError("");
        } catch (err) {
          if (!current()) return;
          clearListingImages();
          setDirectBrowse(null);
          setDirect(null);
          setPublicMode(false);
          setCatalog(null);
          setLoadError(err instanceof Error ? err.message : "The market could not be opened.");
        }
        return;
      }
      try {
        const next = await fetchPublicCatalog(choice.url);
        if (!current()) return;
        clearListingImages();
        setDirectBrowse(choice.url);
        setDirect(choice.url);
        setPublicMode(false);
        setOnionNote(null);
        setCatalog(next);
        setLoadError("");
        try { sessionStorage.setItem(STORE_KEY, choice.url); } catch { /* storage is off */ }
      } catch (err) {
        if (!current()) return;
        setDirectBrowse(null);
        setDirect(null);
        setCatalog(null);
        setLoadError(err instanceof Error ? err.message : "That store could not be opened.");
      }
    } finally {
      if (current()) setLoading(false);
    }
  }
  useEffect(() => { void load(); document.title = "OpenBazaar"; }, []);
  useEffect(() => { if (!nodeAvailable) return; let active = true; const pull = () => { void fetchRates().then(r => { if (active) setRates(r); }); }; pull(); const timer = setInterval(pull, 300000); return () => { active = false; clearInterval(timer); }; }, [nodeAvailable]);
  useEffect(() => { try { localStorage.setItem("ob-shop-saved", JSON.stringify(saved)); } catch {} }, [saved]);
  useEffect(() => { try { localStorage.setItem("ob-price-watched", JSON.stringify(watched)); } catch {} }, [watched]);
  useEffect(() => {
    if (!catalog) return;
    const listings = catalog.listings;
    const drops = priceDrops(watched, listings);
    if (drops.length === 1) setToast(`${drops[0].title} is a lower price than when you saved it`);
    else if (drops.length > 1) setToast(`${drops.length} saved goods are a lower price`);
    const nextWatched = rememberPrices(watched, listings);
    if (!sameWatches(watched, nextWatched)) setWatched(nextWatched);
    const nextSaved = savedHashesAfterRefresh(saved, watched, listings);
    if (nextSaved.some((hash, index) => hash !== saved[index])) setSaved(nextSaved);
  }, [catalog, watched, saved]);
  useEffect(() => { try { localStorage.setItem("ob-sidebar-width", String(sidebarWidth)); localStorage.setItem("ob-sidebar-collapsed", String(collapsed)); } catch {} }, [sidebarWidth, collapsed]);
  useEffect(() => { const update = () => { setMobile(window.innerWidth < 960); setMenu(false); setSettingsOpen(false); }; window.addEventListener("resize", update); return () => window.removeEventListener("resize", update); }, []);
  useEffect(() => { try { localStorage.setItem("ob-shop-cart", JSON.stringify(cart)); } catch {} }, [cart]);
  useEffect(() => { writeSafety(safety); }, [safety]);
  useEffect(() => { if (!nodeAvailable) return; const pull = () => void fetchLabelers().then(setLabelerState, () => setLabelerState(null)); pull(); return afterShopPair(pull); }, [nodeAvailable]);
  const labels = useMemo(() => labelers?.labels ?? [], [labelers]);
  useEffect(() => { if (!loading) content.current?.scrollTo({ top: 0, behavior: "instant" }); }, [loading]);
  useEffect(() => { if (!toastMessage) return; const timer = setTimeout(() => setToast(""), toastAction ? 4000 : 2500); return () => clearTimeout(timer); }, [toast, toastMessage, toastAction]);
  const pageScrollKey = page.name === "order" ? `order:${page.orderId}` : page.name === "sale" ? `sale:${page.saleId}` : page.name;
  useEffect(() => { content.current?.scrollTo({ top: 0, behavior: "instant" }); }, [pageScrollKey, section]);
  const guestRef = useRef(guest);
  guestRef.current = guest;
  useEffect(() => {
    const parsedReceipt = guestRef.current
      ? parseReceiptHash(window.location.hash, window.location.origin)
      : null;
    if (parsedReceipt) {
      window.history.replaceState(null, "", window.location.pathname + window.location.search);
      void createWebBuyerStore().savePhrase(parsedReceipt.phrase);
      setPendingReceiptRestore(parsedReceipt);
    }
    const initialLoc = parsedReceipt
      ? { section: "orders", category: null, page: { name: "browse" as const } }
      : { section: "discover", category: null, page: { name: "browse" as const } };
    const restored = shopHistory.start(initialLoc);
    const loc = sanitizeGuestLocation(restored, guestRef.current);
    if (loc !== restored) shopHistory.replace(loc);
    setSection(loc.section as Section);
    setCategory(loc.category);
    setPage(loc.page);
    const onHashChange = () => {
      if (!guestRef.current) return;
      const nextReceipt = parseReceiptHash(window.location.hash, window.location.origin);
      if (!nextReceipt) return;
      window.history.replaceState(null, "", window.location.pathname + window.location.search);
      void createWebBuyerStore().savePhrase(nextReceipt.phrase);
      setPendingReceiptRestore(nextReceipt);
      const ordersLoc = { section: "orders", category: null, page: { name: "browse" as const } };
      shopHistory.replace(ordersLoc);
      setSection("orders");
      setCategory(null);
      setPage({ name: "browse" });
    };
    window.addEventListener("hashchange", onHashChange);
    const unlisten = shopHistory.listen(next => {
      const safe = sanitizeGuestLocation(next, guestRef.current);
      if (safe !== next) {
        shopHistory.replace(safe);
        return;
      }
      setSection(safe.section as Section);
      setCategory(safe.category);
      setPage(safe.page);
    });
    return () => {
      window.removeEventListener("hashchange", onHashChange);
      unlisten();
    };
  }, []);
  useEffect(() => {
    if (!guest) return;
    const currentLoc = { section, category, page };
    const safe = sanitizeGuestLocation(currentLoc, true);
    if (safe !== currentLoc) shopHistory.replace(safe);
  }, [guest, section, category, page]);
  useEffect(() => { shopHistory.note({ section, category, page }); }, [section, category, page]);
  useEffect(() => {
    if (!menu || !mobile) return;
    const previous = document.activeElement as HTMLElement | null;
    const buttons = () => Array.from(sidebarRef.current?.querySelectorAll<HTMLElement>('button:not([disabled]), select:not([disabled])') ?? []).filter(b => b.offsetParent !== null);
    buttons()[0]?.focus();
    const trap = (e: KeyboardEvent) => { if (e.key !== "Tab") return; const items = buttons(); const first = items[0]; const last = items[items.length - 1]; if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); } else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); } };
    document.addEventListener("keydown", trap);
    return () => { document.removeEventListener("keydown", trap); previous?.focus(); };
  }, [menu, mobile]);
  useEffect(() => { const onKey = (e: KeyboardEvent) => { if ((e.metaKey || e.ctrlKey) && e.key === "k") { e.preventDefault(); if (searchRef.current) searchRef.current.focus(); else { focusSearch.current = true; navigate("discover"); } } if (e.key === "Escape") { if (settingsOpen) { setSettingsOpen(false); settingsToggleRef.current?.focus(); } else setMenu(false); } }; document.addEventListener("keydown", onKey); return () => document.removeEventListener("keydown", onKey); }, [settingsOpen]);
  const allowed = useMemo(() => catalog ? visibleListings(catalog.listings, catalog.store.contentRating, safety, labels) : [], [catalog, safety, labels]);
  const categories = useMemo(() => publicMode ? [...CATEGORY_NAMES] : CATEGORY_NAMES.filter(name => new Set(allowed.map(topCategory)).has(name)), [allowed, publicMode]);
  const savedCount = useMemo(() => allowed.filter(l => saved.includes(l.hash)).length, [allowed, saved]);
  const cartCount = useMemo(() => cart.reduce((n, l) => n + l.quantity, 0), [cart]);
  const visible = useMemo(() => {
    const subset = allowed.filter(l => (!category || topCategory(l) === category) && (!filters.seller || l.seller === filters.seller) && (!filters.kind || l.kind === filters.kind) && (!filters.shipFrom || l.shipFrom === filters.shipFrom) && (section !== "saved" || saved.includes(l.hash)));
    const results = filterListings(subset, query, [], filters);
    return [...results].sort((a, b) => sort === "price-asc" ? a.priceSats - b.priceSats : sort === "price-desc" ? b.priceSats - a.priceSats : sort === "newest" ? b.listedOn - a.listedOn : a.title.localeCompare(b.title));
  }, [catalog, allowed, category, section, saved, query, sort, filters]);
  const paged = useMemo(() => listPage(visible, listPageNum, pageSize), [visible, listPageNum, pageSize]);
  useEffect(() => { setListPageNum(0); }, [category, section, query, sort, filters, pageSize]);
  const filterCount = [filters.seller, filters.kind, filters.shipFrom].filter(Boolean).length + narrowingActive(filters);
  function changeListPage(next: number) {
    setListPageNum(next);
    content.current?.scrollTo({ top: 0, behavior: "instant" });
  }
  function productsPagination(bottom = false) {
    return <nav className="ob-order-pagination" aria-label={`Products pagination${bottom ? " bottom" : ""}`}>
      <button type="button" aria-label="First page" disabled={paged.page === 0} onClick={() => changeListPage(0)}>«</button>
      <button type="button" disabled={paged.page === 0} onClick={() => changeListPage(paged.page - 1)}>Previous</button>
      <span>Page {paged.page + 1} of {paged.pages.toLocaleString()}</span>
      <button type="button" disabled={paged.page + 1 >= paged.pages} onClick={() => changeListPage(paged.page + 1)}>Next</button>
      <button type="button" aria-label="Last page" disabled={paged.page + 1 >= paged.pages} onClick={() => changeListPage(paged.pages - 1)}>»</button>
    </nav>;
  }

  function addToBag(listing: ListingHeader, pick: BuyChoice = { quantity: 1 }) { const line = { hash: listing.hash, variant: pick.variant, storeId: listing.storeId, title: listing.title }; const key = lineKey(line); setCart(lines => { const old = lines.find(l => lineKey(l) === key); const quantity = Math.min(listing.commerce ? 99 : listing.stock, 99, (old?.quantity ?? 0) + pick.quantity); return old ? lines.map(l => lineKey(l) === key ? { ...l, quantity, storeId: listing.storeId, title: listing.title } : l) : [...lines, { ...line, quantity }]; }); setToast({ message: "Added to your bag", action: { label: "View bag", onClick: () => { setToast(""); navigate("bag"); } } }); }
  function navigate(next: Section, nextCategory: string | null = null) {
    setSettingsOpen(false);
    content.current?.scrollTo({ top: 0, behavior: "instant" });
    setFilters(emptyFilters());
    setQuery("");
    setMenu(false);
    setNewListing(0);
    setSection(next);
    setCategory(nextCategory);
    setPage({ name: "browse" });
    shopHistory.section({ section: next, category: nextCategory, page: { name: "browse" } });
  }
  function toggleSaved(listing: ListingHeader) {
    const exists = saved.includes(listing.hash);
    setSaved(a => exists ? a.filter(x => x !== listing.hash) : [...a, listing.hash]);
    setWatched(list => {
      if (!listing.listingId) return list;
      const rest = list.filter(item => item.listingId !== listing.listingId || item.storeId !== listing.storeId);
      if (exists) return rest;
      const price = signedPrice(listing);
      return price ? [...rest, { listingId: listing.listingId, storeId: listing.storeId, hash: listing.hash, amount: price.amount, currency: price.currency }] : rest;
    });
    setToast(exists ? "Removed from saved goods" : "Added to your saved goods");
  }
  function open(listing: ListingHeader, fromStore?: string) { shopHistory.page({ section, category, page: fromStore ? { name: "detail", listing, fromStore } : { name: "detail", listing } }); }
  function openOrder(orderId: string) {
    setSettingsOpen(false);
    setMenu(false);
    shopHistory.page({ section: "orders", category: null, page: { name: "order", orderId } });
  }
  function openSale(saleId: string) {
    setSettingsOpen(false);
    setMenu(false);
    shopHistory.page({ section: "sales", category: null, page: { name: "sale", saleId } });
  }
  const admittedIds = useMemo(() => new Set(admittedStoreIds(directory.data ?? [])), [directory.data]);
  // Empty store banners clutter Products; only surface stores when the buyer is searching by name.
  const namedStores = useMemo(() => {
    if (!catalog || section === "saved" || !query.trim()) return [];
    const withAbout = catalog.stores
      .filter(store => store.key !== storeKey)
      .map(store => ({ ...store, about: store.record.details?.about ?? "" }));
    return matchingStores(withAbout, query);
  }, [catalog, section, query, storeKey]);
  const admittedNamed = useMemo(() => namedStores.filter(store => admittedIds.has(store.key)), [namedStores, admittedIds]);
  const otherNamed = useMemo(() => namedStores.filter(store => !admittedIds.has(store.key)), [namedStores, admittedIds]);
  async function openStore(storeId: string) {
    shopHistory.page({ section, category, page: { name: "store", storeId } });
    if (direct || publicMode) return;
    try {
      // A store the node dropped (unfollowed, or no longer admitted) is not in its shop, so it is not asked for.
      if (!catalog?.stores.some(store => store.key === storeId)) {
        const fresh = await fetchCatalog();
        setCatalog(fresh);
        if (!fresh.stores.some(store => store.key === storeId)) { setToast("That store is no longer in your shop. Follow it to see its goods."); return; }
      }
      const shop = await fetchStore(storeId);
      setOpenedShop({ id: storeId, listings: shop.listings });
      setCatalog(await fetchCatalog());
    } catch (error) { setToast(error instanceof Error ? error.message : "This store could not be opened."); }
  }
  async function startSelling() { try { profile.update(await setRole("seller")); navigate("store"); } catch (e) { setToast(e instanceof Error ? e.message : "Your node could not open a store."); } }
  // A store save must not call load(): that flips `loading` and the content pane scrolls to the top.
  function storeChanged(message: string) {
    setToast(message);
    void refreshCatalog();
    void profile.refresh();
  }
  // The node puts a followed store back in its catalog as it is followed, so the catalog is read again.
  async function followChanged(message: string) {
    setToast(message);
    try { setCatalog(await fetchCatalog()); } catch {}
  }
  async function refreshCatalog() {
    try { setCatalog(await fetchCatalog()); }
    catch (e) { setLoadError(e instanceof Error ? e.message : "Your node could not be reached."); }
  }
  const followable = (storeId: string) => !!profile.data && storeId !== profile.data.storeKey;
  const activeTitle = section === "storefront" ? "Your storefront" : section === "listings" ? "Your listings" : section === "coupons" ? "Coupons" : section === "sales" ? "Sales" : section === "store" ? "Store settings" : section === "follows" ? "Followed stores" : section === "classifieds" ? "Classifieds" : section === "stores" ? "Stores" : section === "saved" ? "Saved goods" : section === "orders" ? "Your orders" : section === "bag" ? "Your bag" : section === "addresses" ? "Your addresses" : section === "wallet" ? "Wallet" : section === "safety" ? "Content filters" : section === "store-labels" ? "Store labels" : section === "home-node" ? "Home node" : section === "browsers" ? "Browsers" : section === "lightning" ? "Lightning" : section === "zcash" ? "Zcash" : section === "monero" ? "Monero" : section === "tor" ? "Tor" : section === "updates" ? "App updates" : "Products";
  const navSection: Section = page.name === "receipt" || page.name === "order" ? "orders" : page.name === "sale" ? "sales" : section;
  const inSettings = ["safety", "addresses", "store-labels", "home-node", "browsers", "lightning", "zcash", "monero", "tor", "updates", "follows"].includes(section);
  const inStore = STORE_SECTIONS.includes(navSection);
  const merchantWorkspace = inStore && (page.name === "browse" || page.name === "sale");
  const browsing = page.name === "browse" && (section === "discover" || (section === "saved" && saved.length > 0));
  const verdictOf = (listing: ListingHeader) => catalog ? safetyVerdict(listing, catalog.store.contentRating, safety, labels) : { action: "show" as const };
  const detailVerdict = page.name === "detail" ? verdictOf(page.listing) : null;

  const money = { rates, currency };
  const utilityView = !guest && (page.name === "order" || (section === "orders" && page.name === "browse"))
    ? <Orders {...money} selectedOrderId={page.name === "order" ? page.orderId : null} unreadByOrder={messageAlerts.unreadOrdersById} onSelectOrder={openOrder} onBack={() => shopHistory.backToParent()} onBrowse={() => navigate("discover")}/>
    : !guest && (page.name === "sale" || (section === "sales" && page.name === "browse"))
      ? storeView()
      : page.name !== "browse" ? null : section === "saved" && saved.length === 0 ? <section><PageHeading title="Saved goods"/><Empty illustration="saved" icon="heart" title="No saved products" text="Save products using the heart button."><button className="ob-primary" onClick={() => navigate("discover")}>Browse products <Icon name="arrow" size={17}/></button></Empty></section> : section === "safety" ? <SafetySettingsPanel settings={safety} onChange={setSafety} labelSources={nodeAvailable ? <LabelersPanel state={labelers} onState={setLabelerState}/> : undefined}/> : guest && section === "orders" ? <TorOrders knownStores={knownVendorStores} catalogLoading={loading} pendingRestore={pendingReceiptRestore} onConsumePendingRestore={() => setPendingReceiptRestore(null)} onBrowse={() => navigate("discover")}/> : guest && section === "classifieds" ? <Classifieds guest/> : guest && section === "stores" ? <GuestStoreDirectory catalog={catalog} onOpen={id => void openStore(id)}/> : guest ? storeView() : section === "wallet" ? <WalletPage {...money} onOrders={() => navigate("orders")} onSales={seller ? () => navigate("sales") : undefined} onManage={rail => navigate(rail)} onTor={() => navigate("tor")}/> : section === "lightning" ? <LightningSettings/> : section === "zcash" ? <ZcashSettings/> : section === "monero" ? <MoneroSettings/> : section === "tor" ? <TorSettings/> : section === "updates" ? <UpdateSettings updater={updater}/> : section === "home-node" ? <div className="ob-settings-page"><header className="ob-settings-heading"><h1>Home node</h1><p>Run your store on an always-on computer and manage it from anywhere over Tor.</p></header><HomeNodePanel/></div> : section === "browsers" ? <BrowsersSettings/> : section === "addresses" ? <Addresses/> : section === "follows" ? <Follows onChanged={message => { setToast(message); void load(); }} onOpen={id => void openStore(id)}/> : section === "classifieds" ? <Classifieds/> : section === "stores" ? <StoreDirectory onOpen={id => void openStore(id)} onFollowChanged={message => void followChanged(message)}/> : storeView();
  function storeView() {
    if (guest && inStore) return <SellGate/>;
    const own = profile.data;
    if (!inStore) return null;
    if (!own) return <p role="status">{profile.error || "Loading your store…"}</p>;
    if (section === "sales" || page.name === "sale") return <Sales sales={sales} {...money} selectedSaleId={page.name === "sale" ? page.saleId : null} unreadBySale={messageAlerts.unreadSalesById} onSelectSale={openSale} onBack={() => shopHistory.backToParent()} onSeen={rememberSales}/>;
    if (section === "listings") return <StoreListings key={newListing} profile={own} {...money} startNew={newListing > 0} onChanged={storeChanged} onOpenSettings={() => navigate("store")}/>;
    if (section === "coupons") return <Coupons onChanged={storeChanged}/>;
    if (section === "storefront" && own.setUp) return <OwnStore profile={own} listings={catalog?.listings ?? []} {...money} onOpen={open} onEdit={() => navigate("store")} onAddListing={() => { navigate("listings"); setNewListing(n => n + 1); }}/>;
    return <StoreSetup key={own.storeKey} profile={own} onSaved={(next, message, viewStore = true) => { profile.update(next); storeChanged(message); if (viewStore) navigate("storefront"); }} onViewStore={() => navigate("storefront")}/>;
  }
  const needsRole = !!profile.data && profile.data.role === null;
  return <NodePaymentsProvider enabled={nodeAvailable} activeWallet={section === "wallet" || section === "orders" || page.name === "receipt" || page.name === "order"} onWallet={() => navigate("wallet")}>{needsRole ? <Onboarding onChosen={next => { profile.update(next); if (next.role === "seller") navigate("store"); }}/> : <div className={`ob-app ${resizing ? "is-resizing" : ""} ${merchantWorkspace ? "is-merchant" : ""}`} style={{ "--ob-sidebar": `${!mobile && collapsed ? 72 : sidebarWidth}px` } as CSSProperties}>
    <a className="ob-skip" href="#ob-content">Skip to content</a>
    {menu && <button className="ob-menu-scrim" aria-label="Close navigation" onClick={() => setMenu(false)}/>}
    <aside ref={sidebarRef} className={`ob-sidebar ${menu ? "is-open" : ""} ${collapsed && !mobile ? "is-collapsed" : ""}`} aria-label="Main navigation" inert={(mobile && !menu)}>
      <button className="ob-sidebar-toggle" onClick={() => setCollapsed(c => !c)} aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"} title={collapsed ? "Expand sidebar" : "Collapse sidebar"}><Icon name="chevron"/></button>
      {mobile && <button className="ob-icon-button ob-drawer-close" aria-label="Close navigation" onClick={() => setMenu(false)}><Icon name="close" size={18}/></button>}
      <div className="ob-sidebar-brand"><button className="ob-brand-button" onClick={() => navigate("discover")} aria-label="OpenBazaar home"><Brand theme={theme}/></button></div>
      <div className="ob-sidebar-scroll">
        <nav className="ob-main-nav" aria-label="Shopping">
          <NavItem icon="compass" active={navSection === "discover" && !category} onClick={() => navigate("discover")}>Products</NavItem>
          <NavItem icon="paper" active={navSection === "classifieds"} onClick={() => navigate("classifieds")}>Classifieds</NavItem>
          <NavItem icon="store" active={navSection === "stores"} onClick={() => navigate("stores")}>Stores</NavItem>
          <NavItem icon="heart" active={navSection === "saved"} onClick={() => navigate("saved")} count={savedCount || undefined}>Saved goods</NavItem>
          <NavItem icon="bag" active={navSection === "bag"} onClick={() => navigate("bag")} count={cartCount || undefined}>Your bag</NavItem>
          {guest && <NavItem icon="paper" active={navSection === "orders"} onClick={() => navigate("orders")} count={guestOrderCount || undefined}>My orders</NavItem>}
        </nav>
        {seller && <><div className="ob-nav-label ob-store-nav-label">Your store</div><nav className="ob-main-nav ob-store-nav" aria-label="Your store">
          <NavItem icon="home" active={navSection === "storefront"} onClick={() => navigate("storefront")}>Storefront</NavItem>
          <NavItem icon="grid" active={navSection === "listings"} onClick={() => navigate("listings")}>Listings</NavItem>
          <NavItem icon="ticket" active={navSection === "coupons"} onClick={() => navigate("coupons")}>Coupons</NavItem>
          <NavItem icon="paper" active={navSection === "sales"} onClick={() => navigate("sales")} count={salesCount?.count} countNew={salesCount?.accent} countLabel={salesCount?.ariaLabel}>Sales</NavItem>
        </nav></>}
        {openDesk && <><div className="ob-nav-label ob-store-nav-label">Moderation</div><nav className="ob-main-nav ob-store-nav" aria-label="Moderation">
          <NavItem icon="shield" active={false} onClick={openDesk}>Moderator desk</NavItem>
        </nav></>}
        {!merchantWorkspace && <><div className="ob-nav-hairline" aria-hidden="true"/>
        <div className="ob-nav-label" id="ob-categories-label">Categories</div>
        <nav className="ob-categories-nav" aria-label="Shopping categories">{categories.map(c => { const count = allowed.filter(l => topCategory(l) === c).length; return <NavItem key={c} icon={categoryIcons.get(c) ?? "grid"} active={navSection === "discover" && category === c} onClick={() => navigate("discover", c)} count={count || undefined}>{c}</NavItem>; })}</nav></>}
      </div>
      <SidebarUtilities
        guest={guest}
        nodeAvailable={nodeAvailable}
        publicMode={publicMode}
        direct={direct}
        loading={loading}
        loadError={loadError}
        section={section}
        navSection={navSection}
        inSettings={inSettings}
        settingsOpen={settingsOpen}
        settingsToggleRef={settingsToggleRef}
        settingsMenuRef={settingsMenuRef}
        onToggleSettings={() => setSettingsOpen(o => !o)}
        onCloseSettings={() => setSettingsOpen(false)}
        onNavigate={next => navigate(next)}
        testnet4={network.data?.network === "testnet4"}
        hasProfile={Boolean(profile.data)}
        seller={seller}
        onStartSelling={() => void startSelling()}
        currency={currency}
        onCurrency={c => { setCurrency(c); try { localStorage.setItem("ob-shop-currency", c); } catch {} }}
        build={build}
        updater={updater}
        onCopied={setToast}
        unreadOrders={messageAlerts.unreadOrders}
      />
      {!collapsed && <div className="ob-sidebar-resize" role="separator" aria-label="Resize sidebar" aria-orientation="vertical" aria-valuemin={224} aria-valuemax={360} aria-valuenow={sidebarWidth} tabIndex={mobile ? -1 : 0} title="Drag to resize. Use arrow keys when focused." onDoubleClick={() => setSidebarWidth(244)} onKeyDown={e => { if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); setSidebarWidth(w => Math.max(224, Math.min(360, w + (e.key === "ArrowRight" ? 16 : -16)))); } if (e.key === "Home") setSidebarWidth(224); if (e.key === "End") setSidebarWidth(360); }} onPointerDown={e => { e.currentTarget.setPointerCapture(e.pointerId); setResizing(true); }} onPointerMove={e => { if (e.currentTarget.hasPointerCapture(e.pointerId)) setSidebarWidth(Math.max(224, Math.min(360, e.clientX))); }} onPointerUp={e => { e.currentTarget.releasePointerCapture(e.pointerId); setResizing(false); }} onPointerCancel={() => setResizing(false)}/>}
    </aside>
    <div className="ob-workspace" inert={(mobile && menu)}>
      <header className="ob-topbar">
        <button className="ob-icon-button ob-mobile-menu" aria-label="Open navigation" aria-expanded={menu} onClick={() => setMenu(true)}><Icon name="menu"/></button>
        <div className="ob-breadcrumb">
          {page.name === "order" ? (
            <>
              <button type="button" className="ob-breadcrumb-link" onClick={() => shopHistory.backToParent()}>Your orders</button>
              <Icon name="chevron" size={12}/>
              <strong>#{page.orderId.slice(0, 8)}</strong>
            </>
          ) : page.name === "sale" ? (
            <>
              <button type="button" className="ob-breadcrumb-link" onClick={() => navigate("storefront")}>Your store</button>
              <Icon name="chevron" size={12}/>
              <button type="button" className="ob-breadcrumb-link" onClick={() => shopHistory.backToParent()}>Sales</button>
              <Icon name="chevron" size={12}/>
              <strong>#{page.saleId.slice(0, 8)}</strong>
            </>
          ) : (
            <>
              {inSettings || inStore || (navSection !== "wallet" && navSection !== "orders") ? <><button type="button" className="ob-breadcrumb-link" aria-current={page.name === "browse" && ((inStore && section === "storefront") || (!inStore && !inSettings && section === "discover" && !category)) ? "page" : undefined} onClick={() => { if (inSettings) { if (mobile) setMenu(true); setSettingsOpen(true); } else if (inStore) navigate("storefront"); else navigate("discover"); }}>{inSettings ? "Settings" : inStore ? "Your store" : "Marketplace"}</button><Icon name="chevron" size={12}/></> : null}
              {section === "stores" && page.name === "store" ? <><button type="button" className="ob-breadcrumb-link" onClick={() => shopHistory.backToParent()}>Stores</button><Icon name="chevron" size={12}/></> : null}
              <strong>{page.name === "browse" ? activeTitle : page.name === "store" ? (catalog?.stores.find(store => store.key === page.storeId)?.name ?? catalog?.listings.find(listing => listing.storeId === page.storeId)?.seller ?? "Store") : page.name === "detail" ? page.listing.title : page.name === "checkout" ? "Checkout" : "Order details"}</strong>
            </>
          )}
        </div>
        <div className="ob-topbar-right">
          {openSignIn && <button className="ob-secondary" type="button" onClick={openSignIn}>Sign in</button>}
          {publicMode ? <div className="ob-network-pill" role="status"><span className="ob-status-dot" aria-hidden="true"/><span>Public</span></div> : direct ? null : nodeAvailable ? <NetworkBadge/> : null}
          <button type="button" className={`ob-topbar-bag${navSection === "bag" ? " is-active" : ""}`} aria-label="Your bag" title="Your bag" aria-current={navSection === "bag" ? "page" : undefined} onClick={() => navigate("bag")}><Icon name="bag" size={17}/>{cartCount > 0 && <span className="ob-topbar-bag-count">{cartCount}</span>}</button>
          <button className="ob-theme-toggle" role="switch" aria-checked={theme === "dark"} aria-label="Dark mode" title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"} onClick={() => setTheme(t => t === "dark" ? "light" : "dark")}><Icon name={theme === "dark" ? "moon" : "sun"} size={17}/><span className="ob-theme-track"><span/></span></button>
        </div>
      </header>
      <HomeNodeBadge/>
      {direct ? <DirectBanner url={direct}/> : null}
      {nodeAvailable && <PairingBanner/>}
      <BuildMismatchWarning build={build}/>
      {section !== "updates" && <UpdateBanner updater={updater}/>}
      <main className="ob-content" id="ob-content" ref={content}>
        <div className="ob-content-inner">
          {browsing && <div className="ob-discovery-bar"><div className="ob-search"><Icon name="search" size={18}/><input ref={searchRef} aria-label="Search stores and listings" placeholder="Search marketplace" value={query} onChange={e => { setQuery(e.target.value); setPage({ name: "browse" }); }}/>{query ? <button className="ob-icon-button" aria-label="Clear search" onClick={() => setQuery("")}><Icon name="close" size={16}/></button> : <kbd>⌘ K</kbd>}</div></div>}
          {utilityView ?? (loading ? <div className="ob-loading" role="status"><span className="ob-spinner"/><h1>{offline || !hasNodeHost() ? "Opening store" : "Loading products"}</h1><p>{offline || !hasNodeHost() ? "Reading public listings…" : "Connecting to your marketplace…"}</p></div> : loadError || !catalog ? <Empty icon="globe" title={offline ? "No node on this device" : "Unable to connect"} text={loadError || "Your node could not be reached."}>{offline && <OpenStoreForm value={storeDraft} busy={loading} onChange={setStoreDraft} onOpen={(url) => void load(url)}/>}<button className="ob-primary" onClick={() => void load()}>Try again <Icon name="arrow" size={16}/></button></Empty> : page.name === "store" ? <CatalogStore storeId={page.storeId} followable={followable(page.storeId)} onFollowChanged={message => void followChanged(message)} store={catalog.stores.find(store => store.key === page.storeId) ?? null} listings={openedShop?.id === page.storeId ? openedShop.listings : allowed.filter(listing => listing.storeId === page.storeId)} {...money} revealed={revealed} onReveal={hash => setRevealed(list => [...list, hash])} verdictOf={verdictOf} onOpen={listing => open(listing, page.storeId)} onStore={openStore} backLabel={section === "stores" ? "Back to stores" : undefined} onBack={() => shopHistory.back()}/> : page.name === "detail" && detailVerdict && detailVerdict.action !== "show" && !revealed.includes(page.listing.hash) ? <SafetyInterstitial reason={detailVerdict.reason} onShow={() => setRevealed(r => [...r, page.listing.hash])} onBack={() => shopHistory.back()} backLabel={page.fromStore ? "Back to store" : "Back to products"}/> : page.name === "detail" ? <><ProductDetail key={page.listing.hash} guest={!!direct || publicMode} own={!!profile.data && page.listing.storeId === profile.data.storeKey} follow={followable(page.listing.storeId) && <FollowButton storeId={page.listing.storeId} onChanged={message => void followChanged(message)}/>} onEdit={() => navigate("listings")} listing={page.listing} alternatives={page.listing.product ? allowed.filter(l => l.product === page.listing.product && l.hash !== page.listing.hash) : []} onAlternative={open} {...money} saved={saved.includes(page.listing.hash)} onSave={() => toggleSaved(page.listing)} onBack={() => shopHistory.back()} backLabel={page.fromStore ? "Back to store" : "Back to products"} onStore={() => openStore(page.listing.storeId)} onAdd={pick => addToBag(page.listing, pick)} onBuy={pick => shopHistory.page({ section, category, page: { name: "checkout", lines: [{ hash: page.listing.hash, variant: pick.variant, quantity: pick.quantity }], returnTo: page.listing, ...(page.fromStore ? { fromStore: page.fromStore } : {}) } })}/><HideActions listing={page.listing} settings={safety} onChange={next => { setSafety(next); shopHistory.backToBrowse(); setToast("Hidden on this device. Undo in Content filters."); }}/></> : page.name === "checkout" && (direct || publicMode) ? (() => { const checkoutListing = page.returnTo ?? (page.lines.length === 1 ? catalog.listings.find(l => l.hash === page.lines[0].hash) : undefined); const checkoutVendorUrl = direct || (checkoutListing ? storeAddresses.get(checkoutListing.storeId) : undefined); return <DirectPay listing={checkoutListing} vendorUrl={checkoutVendorUrl} settlement="direct" onBack={() => shopHistory.back()} backLabel={page.returnTo ? "Back to product" : "Back to bag"}/>; })() : page.name === "checkout" ? <MarketplaceCheckout lines={page.lines} catalog={catalog} {...money} backLabel={page.returnTo ? "Back to product" : "Back to bag"} onBack={() => shopHistory.back()} onDone={receipt => { const bought = new Set(receipt.orders.flatMap(o => o.lines.map(l => l.listingHash))); setCart(c => c.filter(l => !bought.has(l.hash))); shopHistory.replace({ section, category, page: { name: "receipt", receipt } }); }}/> : page.name === "receipt" ? <CheckoutConfirmation receipt={page.receipt} {...money} onBrowse={() => navigate("discover")}/> : section === "bag" ? <Bag catalog={catalog} lines={cart} {...money} onChange={(key, quantity) => setCart(c => quantity === 0 ? c.filter(l => lineKey(l) !== key) : c.map(l => lineKey(l) === key ? { ...l, quantity } : l))} onCheckout={() => shopHistory.page({ section, category, page: { name: "checkout", lines: cart } })} onBrowse={() => navigate("discover")} onRetry={() => void load()}/> : section === "store-labels" ? <div className="ob-settings-page"><header className="ob-settings-heading"><h1>Store labels</h1><p>Set the content ratings buyers see for your store and listings.</p></header><VendorLabelPicker catalog={catalog} onCatalog={setCatalog}/></div> : <>
            <section className="ob-catalog" aria-labelledby="ob-catalog-title"><div className="ob-section-heading"><div><h1 id="ob-catalog-title">{section === "saved" ? "Saved goods" : query ? `Results for “${query}”` : category ? category : "All products"}<span aria-label={`${visible.length} products`}>{visible.length}</span></h1></div><div className="ob-sort-row"><label className="ob-sort">Sort by <select aria-label="Sort goods" value={sort} onChange={e => setSort(e.target.value)}><option value="name">Name</option><option value="newest">Newest first</option><option value="price-asc">Price: low to high</option><option value="price-desc">Price: high to low</option></select></label><label className="ob-sort">Per page <select aria-label="Products per page" value={pageSize} onChange={e => setPageSize(Number(e.target.value))}><option value={24}>24</option><option value={48}>48</option></select></label></div></div>
              <div className="ob-filter-row" role="group" aria-label="Filter by category"><button className={`ob-filter ${!category ? "is-active" : ""}`} aria-pressed={!category} onClick={() => setCategory(null)}><Icon name="grid" size={15}/>All goods</button>{categories.map(c => <button key={c} className={`ob-filter ${category === c ? "is-active" : ""}`} aria-pressed={category === c} onClick={() => setCategory(c)}><Icon name={categoryIcons.get(c) ?? "grid"} size={15}/>{c}</button>)}</div>
              <details className="ob-advanced-filters"><summary>Filters <span>{filterCount ? `${filterCount} active` : "Seller, price & more"}</span><Icon name="chevron" size={13}/></summary><div className="ob-advanced-filter-grid"><label className="ob-field">Seller<select aria-label="Filter by seller" value={filters.seller} onChange={e => setFilters(f => ({ ...f, seller: e.target.value }))}><option value="">All sellers</option>{[...new Set(catalog.listings.map(l => l.seller))].map(s => <option key={s}>{s}</option>)}</select></label><label className="ob-field">Type of good<select aria-label="Filter by type" value={filters.kind} onChange={e => setFilters(f => ({ ...f, kind: e.target.value }))}><option value="">All goods</option><option value="physical">Physical goods</option><option value="digital">Digital goods</option></select></label><label className="ob-field">Ships from<select aria-label="Filter by origin" value={filters.shipFrom} onChange={e => setFilters(f => ({ ...f, shipFrom: e.target.value }))}><option value="">Anywhere</option>{[...new Set(catalog.listings.map(l => l.shipFrom))].map(s => <option key={s}>{s}</option>)}</select></label>{conditionChoices(allowed).length > 0 && <label className="ob-field">Condition<select aria-label="Filter by condition" value={filters.condition} onChange={e => setFilters(f => ({ ...f, condition: e.target.value }))}><option value="">Any condition</option>{conditionChoices(allowed).map(condition => <option key={condition} value={condition}>{CONDITION_LABELS[condition]}</option>)}</select></label>}<label className="ob-field">Min. price in sats<input type="number" min="0" inputMode="numeric" value={filters.min} placeholder="No minimum" onChange={e => setFilters(f => ({ ...f, min: e.target.value }))}/></label><label className="ob-field">Max. price in sats<input type="number" min="0" inputMode="numeric" value={filters.max} placeholder="No maximum" onChange={e => setFilters(f => ({ ...f, max: e.target.value }))}/></label>{specificChoices(allowed).map(group => <label key={group.key} className="ob-field">{group.label}<select aria-label={`Filter by ${group.label}`} value={Object.hasOwn(filters.specifics, group.key) ? filters.specifics[group.key] : ""} onChange={e => setFilters(f => ({ ...f, specifics: setSpecificChoice(f.specifics, group.key, e.target.value) }))}><option value="">{`Any ${group.label.toLowerCase()}`}</option>{group.values.map(value => <option key={value.signed} value={value.signed}>{value.label}</option>)}</select></label>)}<button className="ob-text-link" onClick={() => setFilters(emptyFilters())}>Reset filters</button></div></details>
              {admittedNamed.length > 0 && page.name === "browse" && <div className="ob-store-results"><h2 className="ob-admitted-heading">Stores the directory admitted</h2>{admittedNamed.map(store => <button key={store.key} type="button" className="ob-store-result" onClick={() => void openStore(store.key)}><span className="ob-seller-avatar" aria-hidden="true">{store.name.charAt(0)}</span><span><strong>{store.name}</strong><small>{[store.region, store.tagline].filter(Boolean).join(" · ") || "No goods yet"}</small></span></button>)}</div>}
              {otherNamed.length > 0 && page.name === "browse" && <div className="ob-store-results">{otherNamed.map(store => <button key={store.key} type="button" className="ob-store-result" onClick={() => void openStore(store.key)}><span className="ob-seller-avatar" aria-hidden="true">{store.name.charAt(0)}</span><span><strong>{store.name}</strong><small>{[store.region, store.tagline].filter(Boolean).join(" · ") || "No goods yet"}</small></span></button>)}</div>}
              {visible.length ? <>
                {visible.length > pageSize && <div className="ob-order-list-meta"><p role="status" aria-live="polite">{(paged.start + 1).toLocaleString()}–{Math.min(paged.start + pageSize, visible.length).toLocaleString()} of {visible.length.toLocaleString()} products</p>{productsPagination()}</div>}
                <div className="ob-product-grid">{paged.items.map(listing => { const verdict = verdictOf(listing); return verdict.action === "cover" && !revealed.includes(listing.hash) ? <CoveredCard key={listing.hash} reason={verdict.reason} onShow={() => setRevealed(r => [...r, listing.hash])}/> : <ProductCard key={listing.hash} listing={listing} {...money} saved={saved.includes(listing.hash)} onSave={() => toggleSaved(listing)} onOpen={() => open(listing)} onStore={() => openStore(listing.storeId)}/>; })}</div>
                {visible.length > pageSize && <div className="ob-order-pagination-footer">{productsPagination(true)}</div>}
              </> : namedStores.length ? null : <CatalogEmpty section={section} query={query} category={category} filterCount={filterCount} publicMode={publicMode} onionNote={onionNote} direct={direct} onDiscover={() => navigate("discover")} onResetFilters={() => { setQuery(""); setCategory(null); setFilters(emptyFilters()); if (section === "saved") navigate("discover"); }} onRetry={() => void load()}/>}
            </section>
          </>)}

        </div>
      </main>
    </div>
    {toastMessage && <div className="ob-toast" role="status"><Icon name="check" size={17}/><span>{toastMessage}</span>{toastAction && <button type="button" className="ob-toast-action" onClick={toastAction.onClick}>{toastAction.label}</button>}</div>}
    <PairingDialog/>
  </div>}</NodePaymentsProvider>;
}
export function NavItem({ icon, children, active, count, countNew, countLabel, onClick }: { icon: IconName; children: ReactNode; active: boolean; count?: number; countNew?: boolean; countLabel?: string; onClick: () => void }) { return <button className={`ob-nav-item ${active ? "is-active" : ""}`} aria-label={typeof children === "string" ? children : undefined} title={typeof children === "string" ? children : undefined} aria-current={active ? "page" : undefined} onClick={onClick}><Icon name={icon} size={19}/><span>{children}</span>{count !== undefined && <span className={`ob-nav-count${countNew ? " is-new" : ""}`} aria-label={countLabel}>{count}</span>}</button>; }
export function Empty({ icon, illustration, title, text, children }: { icon: IconName; illustration?: "bag" | "saved"; title: string; text: string; children?: ReactNode }) { return <div className="ob-empty">{illustration ? <MarketEmptyArtwork variant={illustration}/> : <span className="ob-empty-icon"><Icon name={icon} size={30}/></span>}<h2>{title}</h2><p>{text}</p>{children && <div className="ob-empty-actions">{children}</div>}</div>; }
export function ProductImage({ listing, image = listing.images[0], ...props }: { listing: ListingHeader; image?: string; className?: string }) {
  const frame = useRef<HTMLDivElement>(null);
  const [gallery, setGallery] = useState(false);
  const [open, setOpen] = useState(false);
  useEffect(() => { setGallery(Boolean(frame.current?.closest(".ob-gallery"))); }, [image]);
  const src = photo(listing);
  const placeholder = <Icon name={categoryIcons.get(topCategory(listing)) ?? "bag"} size={55}/>;
  return <div ref={frame} className={`ob-product-image ${src || image ? "" : "ob-product-placeholder"} ${props.className ?? ""}`}>{image ? <MediaImage hash={image} alt={listing.title} fallback={placeholder}/> : src ? <img src={src} alt={listing.title} loading="lazy"/> : placeholder}{gallery && image ? <button type="button" className="ob-photo-zoom" aria-label="View photo larger" onClick={() => setOpen(true)}>View larger</button> : null}{open && image ? <PhotoZoom hash={image} title={listing.title} onClose={() => setOpen(false)}/> : null}</div>;
}
export function ProductCard({ listing, rates, currency, saved, onSave, onOpen, onStore }: { listing: ListingHeader; rates: RateTable; currency: CurrencyCode; saved: boolean; onSave?: () => void; onOpen: () => void; onStore?: () => void }) {
  const seller = <><span className="ob-seller-avatar" aria-hidden="true">{listing.seller.charAt(0)}</span>{listing.seller}</>;
  return <article className="ob-product-card"><div className="ob-card-image-wrap"><button className="ob-product-open" onClick={onOpen} aria-label={`View ${listing.title}`}><ProductImage listing={listing}/></button>{onSave && <button className={`ob-save ${saved ? "is-saved" : ""}`} aria-label={`${saved ? "Unsave" : "Save"} ${listing.title}`} aria-pressed={saved} onClick={onSave}><Icon name="heart" size={17}/></button>}{listing.kind === "digital" && <span className="ob-digital-badge"><Icon name="bolt" size={12}/>Digital</span>}{listing.sample && <span className="ob-sample-badge">Sample</span>}</div><div className="ob-card-category">{categoryLabel(listing)}<span>{listing.shipFrom}</span></div><button className="ob-product-title" onClick={onOpen}>{listing.title}</button>{listing.price && <span className="ob-seller-price">{formatPrice(listing.price.amount, listing.price.currency)}</span>}<Money sats={listing.priceSats} rates={rates} currency={currency}/>{onStore ? <button type="button" className="ob-card-seller" onClick={onStore} aria-label={`Open store ${listing.seller}`}>{seller}</button> : <div className="ob-card-seller">{seller}</div>}</article>;
}
function ProductDetail({ listing, guest = false, own, follow, onEdit, rates, currency, saved, onSave, onBack, backLabel = "Back to products", onStore, onBuy, onAdd, alternatives, onAlternative }: { guest?: boolean; own: boolean; follow: ReactNode; onEdit: () => void; alternatives: ListingHeader[]; onAlternative: (listing: ListingHeader) => void; listing: ListingHeader; rates: RateTable; currency: CurrencyCode; saved: boolean; onSave: () => void; onBack: () => void; backLabel?: string; onStore?: () => void; onBuy: (pick: BuyChoice) => void; onAdd: (pick: BuyChoice) => void }) {
  const [alternate, setAlternate] = useState(false);
  const buy = useBuyChoice(listing);
  const [shown, setShown] = useState(listing.images[0]);
  return <section className="ob-detail"><button className="ob-back" onClick={onBack}><Icon name="arrow" size={16}/>{backLabel}</button><div className="ob-detail-grid"><div className="ob-gallery">{listing.images.length ? <><ProductImage listing={listing} image={shown}/>{listing.images.length > 1 && <div className="ob-thumbnails">{listing.images.map((hash, i) => <button key={hash} aria-label={`Show photo ${i + 1}`} aria-pressed={shown === hash} onClick={() => setShown(hash)}><MediaImage hash={hash} alt={`Photo ${i + 1}`}/></button>)}</div>}</> : alternate ? <div className="ob-product-image"><img src="goods/paperback-open.jpg" alt="Paperback opened to its pages"/></div> : <ProductImage listing={listing}/ >}{listing.title === "Paperback novel" && <div className="ob-thumbnails"><button aria-label="Show book cover" aria-pressed={!alternate} onClick={() => setAlternate(false)}><img src="goods/paperback.jpg" alt="Book cover"/></button><button aria-label="Show inside pages" aria-pressed={alternate} onClick={() => setAlternate(true)}><img src="goods/paperback-open.jpg" alt="Inside pages"/></button></div>}</div><div className="ob-detail-copy"><span className="ob-eyebrow">{categoryLabel(listing)} · {listing.kind === "digital" ? "Digital" : "Physical item"}</span><h1>{listing.title}</h1><div className="ob-detail-seller-row">{onStore ? <button type="button" className="ob-detail-seller" onClick={onStore} aria-label={`Open store ${listing.seller}`}><span className="ob-seller-avatar" aria-hidden="true">{listing.seller.charAt(0)}</span><span>Sold by <strong>{listing.seller}</strong></span></button> : <div className="ob-detail-seller"><span className="ob-seller-avatar">{listing.seller.charAt(0)}</span><span>Sold by <strong>{listing.seller}</strong></span></div>}{follow}</div>{listing.commerce ? <ListingBuyTerms listing={listing} buy={buy} rates={rates}/> : <><Money sats={listing.priceSats} rates={rates} currency={currency} large/><p className="ob-btc-price">{btc(listing.priceSats)}{listing.price && ` · Seller's price ${formatPrice(listing.price.amount, listing.price.currency)}${listing.price.shipping ? ` + ${formatPrice(listing.price.shipping, listing.price.currency)} shipping` : ""}, paid in sats at the rate when you order`}</p></>}<p className="ob-description">{listing.description || "No description provided."}</p><div className="ob-product-facts"><span><Icon name={listing.kind === "digital" ? "bolt" : "pin"} size={18}/>{listing.kind === "digital" ? "Delivered digitally" : `Ships from ${listing.shipFrom}`}</span>{!listing.commerce && <span><span className={`ob-status-dot ${listing.stock <= 0 ? "is-offline" : ""}`}/>{listing.stock > 0 ? `${listing.stock} available` : "Currently out of stock"}</span>}</div><ListingBuyFacts listing={listing}/>{own ? <div className="ob-detail-actions"><p className="ob-page-intro">This is your listing.</p><button className="ob-secondary" onClick={onEdit}>Edit in your listings <Icon name="arrow" size={18}/></button></div> : <div className="ob-detail-actions">{(!listing.commerce || !listing.price) && <QuantityStepper quantity={buy.quantity} max={buy.max} onQuantity={buy.setQuantity}/>}<button className="ob-primary" disabled={!buy.canBuy} onClick={() => onAdd(buy.choice)}>Add to bag <Icon name="bag" size={18}/></button><button className="ob-secondary" disabled={!buy.canBuy} onClick={() => onBuy(buy.choice)}>Buy now <Icon name="arrow" size={18}/></button><button className={`ob-secondary ob-save-detail ${saved ? "is-saved" : ""}`} aria-pressed={saved} onClick={onSave}><Icon name="heart" size={18}/>{saved ? "Saved" : "Save for later"}</button>{guest ? <p className="ob-shipping-note">{PAYMENT_NOTE}</p> : <OfferSheet listing={listing} choice={buy.choice}/>}</div>}<ListingRails listing={listing}/><ListingReviews listing={listing} own={own}/>{alternatives.length > 0 && <div className="ob-other-sellers"><h2>Other sellers</h2>{alternatives.map(other => <button key={other.hash} onClick={() => onAlternative(other)}><span><strong>{other.seller}</strong><small>Ships from {other.shipFrom}</small></span><Money sats={other.priceSats} rates={rates} currency={currency}/><Icon name="arrow" size={16}/></button>)}</div>}</div></div></section>;}
export function Payment({ receipt, hideStatus = false }: { receipt: BuyerOrder | import("../catalog").SellerOrder; hideStatus?: boolean }) {
  const id = "id" in receipt ? receipt.id : receipt.orderId;
  if (receipt.payment?.ln) return <LightningOrderPayment key={id} id={id} initial={receipt} invoice={receipt.payment.ln} hideStatus={hideStatus}/>;
  return <OrderPayment key={id} id={id} initial={receipt} hideStatus={hideStatus}/>;
}
