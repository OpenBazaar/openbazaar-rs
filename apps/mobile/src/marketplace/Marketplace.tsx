import { useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { fetchCatalog, fetchLabelers, fetchOrders, fetchRates, type LabelerState, type BuyerOrder, type CurrencyCode, type ListingHeader, type RateTable, type ShopCatalog } from "../catalog";
import { filterListings } from "../search";
import { Bag, Addresses, MarketplaceCheckout, CheckoutConfirmation } from "./Commerce";
import { lineKey, type CartLine } from "../cart";
import { ListingBuyFacts, ListingBuyTerms, useBuyChoice, type BuyChoice } from "./ListingBuy";
import { type CheckoutReceipt } from "../catalog";
import { safetyVerdict, visibleListings } from "../safety/filter";
import { readSafety, writeSafety } from "../safety/storage";
import { CoveredCard, HideActions, SafetyInterstitial, SafetySettingsPanel } from "./Safety";
import { VendorLabelPicker } from "./VendorLabelPicker";
import { LabelersPanel } from "./Labelers";
import { CanopyDivider, MarketEmptyArtwork } from "./MarketArtwork";
import { NetworkBadge, NodePaymentsProvider, OrderPayment, WalletPanel } from "./NodePayments";
import { useNodeResource } from "../useNodeResource";
import { PageHeading } from "./PageHeading";
import { BuildLine, BuildMismatchWarning, useBuildInfo } from "./BuildInfo";
import { HomeNodePanel } from "./HomeNodePanel";
import { HomeNodeBadge } from "./HomeNodeGate";
import { LightningSettings } from "./LightningSettings";
import { LightningOrderPayment, LightningWalletCard } from "./LightningPayment";
import { fetchProfile, formatPrice, salesToShip, setRole } from "../seller";
import { Onboarding } from "./Onboarding";
import { StoreSetup } from "./StoreSetup";
import { StoreListings } from "./StoreListings";
import { Sales, useSales } from "./Sales";
import { OwnStore } from "./OwnStore";
import { Follows } from "./Follows";
import { RemoteOrderStatus } from "./RemoteOrder";
import { MediaImage } from "./SellerKit";


type IconName = "settings" | "wallet" | "sun" | "moon" | "compass" | "heart" | "bag" | "arrow" | "search" | "chevron" | "check" | "shield" | "globe" | "close" | "menu" | "book" | "cup" | "home" | "music" | "shirt" | "tool" | "paper" | "grid" | "pin" | "bolt" | "external";
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
  };
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" {...props}>{paths[name]}</svg>;
}
const categoryIcons: Record<string, IconName> = { Books: "book", Grocery: "cup", Home: "home", Music: "music", Clothing: "shirt", Tools: "tool", Paper: "paper" };
const photos: Record<string, string> = { "Paperback novel": "paperback", "Field guide to tide pools": "paperback-open", "Whole bean coffee, 12 oz": "coffee", "Wildflower honey, 8 oz": "honey", "Linen dish towel": "towel", "Stoneware mug": "mug", "Wool socks": "socks", "Clothbound notebook": "notebook", Sketchbook: "notebook", "Beeswax candles, set of 4": "candles", "Cotton work shirt": "shirt", "Work shirt": "shirt", "Cotton apron": "shirt", "Garden trowel": "trowel", "Letterpress cards": "cards" };
function photo(listing: ListingHeader) { return photos[listing.title] ? `goods/${photos[listing.title]}.jpg` : null; }
function readSaved(): string[] { try { const a = JSON.parse(localStorage.getItem("ob-shop-saved") ?? "[]"); return Array.isArray(a) ? a.filter((s): s is string => typeof s === "string") : []; } catch { return []; } }
function readCurrency(): CurrencyCode { try { const c = localStorage.getItem("ob-shop-currency"); return ["USD", "EUR", "GBP", "JPY"].includes(c ?? "") ? c as CurrencyCode : "USD"; } catch { return "USD"; } }
function readSidebarWidth() { try { const n = Number(localStorage.getItem("ob-sidebar-width")); return n >= 224 && n <= 360 ? n : 244; } catch { return 244; } }
function readCollapsed() { try { return localStorage.getItem("ob-sidebar-collapsed") === "true"; } catch { return false; } }
function fiat(sats: number, rates: RateTable, currency: CurrencyCode) { const r = rates[currency]; return r ? new Intl.NumberFormat("en-US", { style: "currency", currency }).format(sats / 1e8 * r) : null; }
function btc(sats: number) { return `${(sats / 1e8).toFixed(8).replace(/0+$/, "").replace(/\.$/, "")} BTC`; }
export function Money({ sats, rates, currency, large = false }: { sats: number; rates: RateTable; currency: CurrencyCode; large?: boolean }) { const estimate = fiat(sats, rates, currency); return <div className={`ob-money ${large ? "ob-money-large" : ""}`}><span>{sats.toLocaleString("en-US")} <small>sats</small></span>{estimate && <span className="ob-estimate" title={`Estimated value in ${currency}`}>≈ {estimate}</span>}</div>; }
function Brand() { return <span className="ob-brand"><span className="ob-brand-icon"><img src="brand/ob-market-logo-256.png" alt="" width={42} height={42}/></span><span>OpenBazaar<span className="ob-brand-dot">.</span></span></span>; }

type Section = "discover" | "saved" | "orders" | "bag" | "addresses" | "safety" | "wallet" | "store-labels" | "home-node" | "lightning" | "follows" | StoreSection;
type StoreSection = "storefront" | "listings" | "sales" | "store";
const STORE_SECTIONS: readonly Section[] = ["storefront", "listings", "sales", "store"];
type Page = { name: "browse" } | { name: "detail"; listing: ListingHeader } | { name: "checkout"; lines: CartLine[]; returnTo?: ListingHeader } | { name: "receipt"; receipt: CheckoutReceipt };

export default function Marketplace() {
  const [catalog, setCatalog] = useState<ShopCatalog | null>(null);
  const [loadError, setLoadError] = useState("");
  const [loading, setLoading] = useState(true);
  const [rates, setRates] = useState<RateTable>({});
  const [currency, setCurrency] = useState<CurrencyCode>(readCurrency);
  const [section, setSection] = useState<Section>("discover");
  const [page, setPage] = useState<Page>({ name: "browse" });
  const [category, setCategory] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState("newest");
  const [filters, setFilters] = useState({ seller: "", kind: "", shipFrom: "", min: "", max: "" });
  const [saved, setSaved] = useState<string[]>(readSaved);
  const [cart, setCart] = useState<CartLine[]>(() => { try { const a = JSON.parse(localStorage.getItem("ob-shop-cart") ?? "[]"); return Array.isArray(a) ? a.filter(l => typeof l.hash === "string" && Number.isInteger(l.quantity) && l.quantity > 0 && l.quantity <= 99) : []; } catch { return []; } });
  const [menu, setMenu] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [theme, setTheme] = useState<"light" | "dark">(() => { try { const t = localStorage.getItem("ob-theme"); if (t === "light" || t === "dark") return t; } catch {} return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"; });
  useEffect(() => { document.documentElement.dataset.obTheme = theme; try { localStorage.setItem("ob-theme", theme); } catch {} }, [theme]);
  const [toast, setToast] = useState("");
  const build = useBuildInfo();
  const profile = useNodeResource(fetchProfile, 0);
  const seller = profile.data?.role === "seller";
  const sales = useSales(seller, sale => setToast(`New order: ${sale.lines.map(l => l.title).join(", ")}`));
  const toShip = salesToShip(sales.data ?? []);
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
  async function load() { setLoading(true); setLoadError(""); try { setCatalog(await fetchCatalog()); } catch (e) { setLoadError(e instanceof Error ? e.message : "Your node could not be reached."); } finally { setLoading(false); } }
  useEffect(() => { void load(); const link = document.createElement("link"); link.rel = "stylesheet"; link.href = "marketplace.css"; document.head.appendChild(link); const sellerCss = document.createElement("link"); sellerCss.rel = "stylesheet"; sellerCss.href = "seller.css"; document.head.appendChild(sellerCss); document.title = "OpenBazaar"; return () => { link.remove(); sellerCss.remove(); }; }, []);
  useEffect(() => { let active = true; const pull = () => { void fetchRates().then(r => { if (active) setRates(r); }); }; pull(); const timer = setInterval(pull, 300000); return () => { active = false; clearInterval(timer); }; }, []);
  useEffect(() => { try { localStorage.setItem("ob-shop-saved", JSON.stringify(saved)); } catch {} }, [saved]);
  useEffect(() => { try { localStorage.setItem("ob-sidebar-width", String(sidebarWidth)); localStorage.setItem("ob-sidebar-collapsed", String(collapsed)); } catch {} }, [sidebarWidth, collapsed]);
  useEffect(() => { const update = () => { setMobile(window.innerWidth < 960); setMenu(false); setSettingsOpen(false); }; window.addEventListener("resize", update); return () => window.removeEventListener("resize", update); }, []);
  useEffect(() => { try { localStorage.setItem("ob-shop-cart", JSON.stringify(cart)); } catch {} }, [cart]);
  useEffect(() => { writeSafety(safety); }, [safety]);
  useEffect(() => { void fetchLabelers().then(setLabelerState, () => setLabelerState(null)); }, []);
  const labels = useMemo(() => labelers?.labels ?? [], [labelers]);
  useEffect(() => { if (!loading) content.current?.scrollTo({ top: 0, behavior: "instant" }); }, [loading]);
  useEffect(() => { if (!toast) return; const timer = setTimeout(() => setToast(""), 2500); return () => clearTimeout(timer); }, [toast]);
  useEffect(() => { content.current?.scrollTo({ top: 0, behavior: "instant" }); }, [page.name, section]);
  useEffect(() => {
    if (!menu || !mobile) return;
    const previous = document.activeElement as HTMLElement | null;
    const buttons = () => Array.from(sidebarRef.current?.querySelectorAll<HTMLButtonElement>('button:not([disabled])') ?? []).filter(b => b.offsetParent !== null);
    buttons()[0]?.focus();
    const trap = (e: KeyboardEvent) => { if (e.key !== "Tab") return; const items = buttons(); const first = items[0]; const last = items[items.length - 1]; if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); } else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); } };
    document.addEventListener("keydown", trap);
    return () => { document.removeEventListener("keydown", trap); previous?.focus(); };
  }, [menu, mobile]);
  useEffect(() => { const onKey = (e: KeyboardEvent) => { if ((e.metaKey || e.ctrlKey) && e.key === "k") { e.preventDefault(); if (searchRef.current) searchRef.current.focus(); else { focusSearch.current = true; navigate("discover"); } } if (e.key === "Escape") { if (settingsOpen) { setSettingsOpen(false); settingsToggleRef.current?.focus(); } else setMenu(false); } }; document.addEventListener("keydown", onKey); return () => document.removeEventListener("keydown", onKey); }, [settingsOpen]);
  const allowed = useMemo(() => catalog ? visibleListings(catalog.listings, catalog.store.contentRating, safety, labels) : [], [catalog, safety, labels]);
  const categories = useMemo(() => [...new Set(allowed.map(l => l.category))], [allowed]);
  const savedCount = useMemo(() => allowed.filter(l => saved.includes(l.hash)).length, [allowed, saved]);
  const visible = useMemo(() => {
    const subset = allowed.filter(l => (!category || l.category === category) && (!filters.seller || l.seller === filters.seller) && (!filters.kind || l.kind === filters.kind) && (!filters.shipFrom || l.shipFrom === filters.shipFrom) && (!filters.min || l.priceSats >= Number(filters.min)) && (!filters.max || l.priceSats <= Number(filters.max)) && (section !== "saved" || saved.includes(l.hash)));
    const results = filterListings(subset, query);
    return [...results].sort((a, b) => sort === "price-asc" ? a.priceSats - b.priceSats : sort === "price-desc" ? b.priceSats - a.priceSats : sort === "newest" ? b.listedOn - a.listedOn : a.title.localeCompare(b.title));
  }, [catalog, allowed, category, section, saved, query, sort, filters]);

  function addToBag(listing: ListingHeader, pick: BuyChoice = { quantity: 1 }) { const line = { hash: listing.hash, variant: pick.variant }; const key = lineKey(line); setCart(lines => { const old = lines.find(l => lineKey(l) === key); const quantity = Math.min(listing.commerce ? 99 : listing.stock, 99, (old?.quantity ?? 0) + pick.quantity); return old ? lines.map(l => lineKey(l) === key ? { ...l, quantity } : l) : [...lines, { ...line, quantity }]; }); setToast("Added to your bag"); }
  function navigate(next: Section, nextCategory: string | null = null) { setSettingsOpen(false); content.current?.scrollTo({ top: 0, behavior: "instant" }); setSection(next); setCategory(nextCategory); setFilters({ seller: "", kind: "", shipFrom: "", min: "", max: "" }); setQuery(""); setPage({ name: "browse" }); setMenu(false); setNewListing(0); }
  function toggleSaved(listing: ListingHeader) { const exists = saved.includes(listing.hash); setSaved(a => exists ? a.filter(x => x !== listing.hash) : [...a, listing.hash]); setToast(exists ? "Removed from saved goods" : "Added to your saved goods"); }
  function open(listing: ListingHeader) { setPage({ name: "detail", listing }); }
  async function startSelling() { try { profile.update(await setRole("seller")); navigate("store"); } catch (e) { setToast(e instanceof Error ? e.message : "Your node could not open a store."); } }
  function storeChanged(message: string) { setToast(message); void load(); void profile.refresh(); }
  const activeTitle = section === "storefront" ? "Your storefront" : section === "listings" ? "Your listings" : section === "sales" ? "Sales" : section === "store" ? "Store settings" : section === "follows" ? "Followed stores" : section === "saved" ? "Saved goods" : section === "orders" ? "Your orders" : section === "bag" ? "Your bag" : section === "addresses" ? "Your addresses" : section === "wallet" ? "Wallet" : section === "safety" ? "Content filters" : section === "store-labels" ? "Store labels" : section === "home-node" ? "Home node" : section === "lightning" ? "Lightning" : "Products";
  const inSettings = ["safety", "addresses", "store-labels", "home-node", "lightning", "follows"].includes(section);
  const inStore = STORE_SECTIONS.includes(section);
  const browsing = page.name === "browse" && (section === "discover" || (section === "saved" && saved.length > 0));
  const verdictOf = (listing: ListingHeader) => catalog ? safetyVerdict(listing, catalog.store.contentRating, safety, labels) : { action: "show" as const };
  const detailVerdict = page.name === "detail" ? verdictOf(page.listing) : null;

  const money = { rates, currency };
  const navSection: Section = page.name === "receipt" ? "orders" : section;
  const utilityView = page.name !== "browse" ? null : section === "saved" && saved.length === 0 ? <section><PageHeading title="Saved goods"/><Empty illustration="saved" icon="heart" title="No saved products" text="Save products using the heart button."><button className="ob-primary" onClick={() => navigate("discover")}>Browse products <Icon name="arrow" size={17}/></button></Empty></section> : section === "wallet" ? <><WalletPanel {...money}/><LightningWalletCard {...money}/></> : section === "lightning" ? <LightningSettings/> : section === "home-node" ? <div className="ob-settings-page"><header className="ob-settings-heading"><h1>Home node</h1><p>Run your store on an always-on computer and manage it from anywhere over Tor.</p></header><HomeNodePanel/></div> : section === "addresses" ? <Addresses/> : section === "safety" ? <SafetySettingsPanel settings={safety} onChange={setSafety} labelSources={<LabelersPanel state={labelers} onState={setLabelerState}/>}/> : section === "orders" ? <Orders {...money} onBrowse={() => navigate("discover")}/> : section === "follows" ? <Follows onChanged={message => { setToast(message); void load(); }}/> : storeView();
  function storeView() {
    const own = profile.data;
    if (!inStore) return null;
    if (!own) return <p role="status">{profile.error || "Loading your store…"}</p>;
    if (section === "sales") return <Sales sales={sales} {...money}/>;
    if (section === "listings") return <StoreListings key={newListing} profile={own} {...money} startNew={newListing > 0} onChanged={storeChanged}/>;
    if (section === "storefront" && own.setUp) return <OwnStore profile={own} listings={catalog?.listings ?? []} {...money} onOpen={open} onEdit={() => navigate("store")} onAddListing={() => { navigate("listings"); setNewListing(n => n + 1); }}/>;
    return <StoreSetup key={own.storeKey} profile={own} onSaved={(next, message) => { profile.update(next); storeChanged(message); navigate("storefront"); }} onViewStore={() => navigate("storefront")}/>;
  }
  const needsRole = !!profile.data && profile.data.role === null;
  return <NodePaymentsProvider activeWallet={section === "wallet" || section === "orders" || page.name === "receipt"} onWallet={() => navigate("wallet")}>{needsRole ? <Onboarding onChosen={next => { profile.update(next); if (next.role === "seller") navigate("store"); }}/> : <div className={`ob-app ${resizing ? "is-resizing" : ""}`} style={{ "--ob-sidebar": `${!mobile && collapsed ? 72 : sidebarWidth}px` } as CSSProperties}>
    <a className="ob-skip" href="#ob-content">Skip to content</a>
    {menu && <button className="ob-menu-scrim" aria-label="Close navigation" onClick={() => setMenu(false)}/>}
    <aside ref={sidebarRef} className={`ob-sidebar ${menu ? "is-open" : ""} ${collapsed && !mobile ? "is-collapsed" : ""}`} aria-label="Main navigation" inert={(mobile && !menu)}>
      <button className="ob-sidebar-toggle" onClick={() => setCollapsed(c => !c)} aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"} title={collapsed ? "Expand sidebar" : "Collapse sidebar"}><Icon name="chevron"/></button>
      {mobile && <button className="ob-icon-button ob-drawer-close" aria-label="Close navigation" onClick={() => setMenu(false)}><Icon name="close" size={18}/></button>}
      <div className="ob-sidebar-brand"><button className="ob-brand-button" onClick={() => navigate("discover")} aria-label="OpenBazaar home"><Brand/></button></div>
      <div className="ob-sidebar-scroll">
        <nav className="ob-main-nav" aria-label="Shopping">
          <NavItem icon="compass" active={navSection === "discover" && !category} onClick={() => navigate("discover")}>Products</NavItem>
          <NavItem icon="heart" active={navSection === "saved"} onClick={() => navigate("saved")} count={savedCount || undefined}>Saved goods</NavItem>
          <NavItem icon="bag" active={navSection === "bag"} onClick={() => navigate("bag")} count={cart.reduce((n, l) => n + l.quantity, 0) || undefined}>Your bag</NavItem>
        </nav>
        {seller && <><div className="ob-nav-label ob-store-nav-label">Your store</div><nav className="ob-main-nav ob-store-nav" aria-label="Your store">
          <NavItem icon="home" active={navSection === "storefront"} onClick={() => navigate("storefront")}>Storefront</NavItem>
          <NavItem icon="grid" active={navSection === "listings"} onClick={() => navigate("listings")}>Listings</NavItem>
          <NavItem icon="paper" active={navSection === "sales"} onClick={() => navigate("sales")} count={toShip || undefined}>Sales</NavItem>
        </nav></>}
        <CanopyDivider/>
        <div className="ob-nav-label" id="ob-categories-label">Categories</div>
        <nav className="ob-categories-nav" aria-label="Shopping categories">{categories.map(c => <NavItem key={c} icon={categoryIcons[c] ?? "grid"} active={navSection === "discover" && category === c} onClick={() => navigate("discover", c)} count={allowed.filter(l => l.category === c).length}>{c}</NavItem>)}</nav>
      </div>
      <div className="ob-sidebar-utilities">
        <nav className="ob-utility-nav" aria-label="Orders and wallet">
          <NavItem icon="paper" active={navSection === "orders"} onClick={() => navigate("orders")}>Your orders</NavItem>
          <NavItem icon="wallet" active={navSection === "wallet"} onClick={() => navigate("wallet")}>Wallet</NavItem>
        </nav>
        <button ref={settingsToggleRef} className={`ob-nav-item ob-settings-toggle ${inSettings ? "has-active-child" : ""}`} aria-label="Settings" title="Settings" aria-expanded={settingsOpen} aria-controls="ob-settings-nav" onClick={() => setSettingsOpen(o => !o)}><Icon name="settings" size={19}/><span>Settings</span><Icon name="chevron" size={14}/></button>
        <nav ref={settingsMenuRef} id="ob-settings-nav" className="ob-settings-nav" aria-label="Settings" hidden={!settingsOpen} onBlur={e => { if (!e.currentTarget.contains(e.relatedTarget) && e.relatedTarget !== settingsToggleRef.current) setSettingsOpen(false); }}>
          <div className="ob-settings-menu-title">Settings</div>
          <NavItem icon="shield" active={section === "safety"} onClick={() => navigate("safety")}>Content filters</NavItem>
          <NavItem icon="pin" active={section === "addresses"} onClick={() => navigate("addresses")}>Addresses</NavItem>
          <NavItem icon="grid" active={section === "store-labels"} onClick={() => navigate("store-labels")}>Store labels</NavItem>
          <NavItem icon="globe" active={section === "home-node"} onClick={() => navigate("home-node")}>Home node</NavItem>
          <NavItem icon="bolt" active={section === "lightning"} onClick={() => navigate("lightning")}>Lightning</NavItem>
          <NavItem icon="globe" active={section === "follows"} onClick={() => navigate("follows")}>Followed stores</NavItem>
          {profile.data && (seller ? <NavItem icon="home" active={section === "store"} onClick={() => navigate("store")}>Store settings</NavItem> : <NavItem icon="home" active={false} onClick={() => void startSelling()}>Start selling</NavItem>)}
        </nav>
        <div className="ob-node" title={loading ? "Connecting to your node" : loadError ? "Your node is offline" : "Your node is connected"}><span className={`ob-status-dot ${loadError ? "is-offline" : loading ? "is-loading" : ""}`}/><div><strong>{loading ? "Connecting to your node" : loadError ? "Your node is offline" : "Your node is connected"}</strong></div></div>
        <BuildLine build={build} onCopied={setToast}/>
      </div>
      {!collapsed && <div className="ob-sidebar-resize" role="separator" aria-label="Resize sidebar" aria-orientation="vertical" aria-valuemin={224} aria-valuemax={360} aria-valuenow={sidebarWidth} tabIndex={mobile ? -1 : 0} title="Drag to resize. Use arrow keys when focused." onDoubleClick={() => setSidebarWidth(244)} onKeyDown={e => { if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); setSidebarWidth(w => Math.max(224, Math.min(360, w + (e.key === "ArrowRight" ? 16 : -16)))); } if (e.key === "Home") setSidebarWidth(224); if (e.key === "End") setSidebarWidth(360); }} onPointerDown={e => { e.currentTarget.setPointerCapture(e.pointerId); setResizing(true); }} onPointerMove={e => { if (e.currentTarget.hasPointerCapture(e.pointerId)) setSidebarWidth(Math.max(224, Math.min(360, e.clientX))); }} onPointerUp={e => { e.currentTarget.releasePointerCapture(e.pointerId); setResizing(false); }} onPointerCancel={() => setResizing(false)}/>}
    </aside>
    <div className="ob-workspace" inert={(mobile && menu)}>
      <header className="ob-topbar">
        <button className="ob-icon-button ob-mobile-menu" aria-label="Open navigation" aria-expanded={menu} onClick={() => setMenu(true)}><Icon name="menu"/></button>
        <div className="ob-breadcrumb"><span>{inSettings ? "Settings" : inStore ? "Your store" : section === "wallet" || section === "orders" ? "Your activity" : "Marketplace"}</span><Icon name="chevron" size={12}/><strong>{page.name === "browse" ? activeTitle : page.name === "detail" ? page.listing.title : page.name === "checkout" ? "Checkout" : "Order details"}</strong></div>
        <div className="ob-topbar-right"><button className="ob-theme-toggle" role="switch" aria-checked={theme === "dark"} aria-label="Dark mode" title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"} onClick={() => setTheme(t => t === "dark" ? "light" : "dark")}><Icon name={theme === "dark" ? "moon" : "sun"} size={17}/><span className="ob-theme-track"><span/></span></button><label className="ob-currency"><Icon name="globe" size={16}/><select aria-label="Display currency" value={currency} onChange={e => { const c = e.target.value as CurrencyCode; setCurrency(c); try { localStorage.setItem("ob-shop-currency", c); } catch {} }}>{["USD", "EUR", "GBP", "JPY"].map(c => <option key={c}>{c}</option>)}</select></label></div>
      </header>
      <HomeNodeBadge/>
      <NetworkBadge/>
      <BuildMismatchWarning build={build}/>
      <main className="ob-content" id="ob-content" ref={content}>
        <div className="ob-content-inner">
          {browsing && <div className="ob-discovery-bar"><div className="ob-search"><Icon name="search" size={18}/><input ref={searchRef} aria-label="Search listings" placeholder="Search marketplace" value={query} onChange={e => { setQuery(e.target.value); setPage({ name: "browse" }); }}/>{query ? <button className="ob-icon-button" aria-label="Clear search" onClick={() => setQuery("")}><Icon name="close" size={16}/></button> : <kbd>⌘ K</kbd>}</div></div>}
          {utilityView ?? (loading ? <div className="ob-loading" role="status"><span className="ob-spinner"/><h1>Loading products</h1><p>Connecting to your marketplace…</p></div> : loadError || !catalog ? <Empty icon="globe" title="Unable to connect" text={loadError || "Your node could not be reached."}><button className="ob-primary" onClick={() => void load()}>Try again <Icon name="arrow" size={16}/></button></Empty> : page.name === "detail" && detailVerdict && detailVerdict.action !== "show" && !revealed.includes(page.listing.hash) ? <SafetyInterstitial reason={detailVerdict.reason} onShow={() => setRevealed(r => [...r, page.listing.hash])} onBack={() => setPage({ name: "browse" })}/> : page.name === "detail" ? <><ProductDetail key={page.listing.hash} own={!!profile.data && page.listing.storeId === profile.data.storeKey} onEdit={() => navigate("listings")} listing={page.listing} alternatives={page.listing.product ? allowed.filter(l => l.product === page.listing.product && l.hash !== page.listing.hash) : []} onAlternative={open} {...money} saved={saved.includes(page.listing.hash)} onSave={() => toggleSaved(page.listing)} onBack={() => setPage({ name: "browse" })} onAdd={pick => addToBag(page.listing, pick)} onBuy={pick => setPage({ name: "checkout", lines: [{ hash: page.listing.hash, variant: pick.variant, quantity: pick.quantity }], returnTo: page.listing })}/><HideActions listing={page.listing} settings={safety} onChange={next => { setSafety(next); setPage({ name: "browse" }); setToast("Hidden on this device. Undo in Content filters."); }}/></> : page.name === "checkout" ? <MarketplaceCheckout lines={page.lines} catalog={catalog} {...money} backLabel={page.returnTo ? "Back to product" : "Back to bag"} onBack={() => page.returnTo ? setPage({ name: "detail", listing: page.returnTo }) : navigate("bag")} onDone={receipt => { const bought = new Set(receipt.orders.flatMap(o => o.lines.map(l => l.listingHash))); setCart(c => c.filter(l => !bought.has(l.hash))); setPage({ name: "receipt", receipt }); }}/> : page.name === "receipt" ? <CheckoutConfirmation receipt={page.receipt} {...money} onBrowse={() => navigate("discover")}/> : section === "bag" ? <Bag catalog={catalog} lines={cart} {...money} onChange={(key, quantity) => setCart(c => quantity === 0 ? c.filter(l => lineKey(l) !== key) : c.map(l => lineKey(l) === key ? { ...l, quantity } : l))} onCheckout={() => setPage({ name: "checkout", lines: cart })} onBrowse={() => navigate("discover")}/> : section === "store-labels" ? <div className="ob-settings-page"><header className="ob-settings-heading"><h1>Store labels</h1><p>Set the content ratings buyers see for your store and listings.</p></header><VendorLabelPicker catalog={catalog} onCatalog={setCatalog}/></div> : <>
            <section className="ob-catalog" aria-labelledby="ob-catalog-title"><div className="ob-section-heading"><div><h1 id="ob-catalog-title">{section === "saved" ? "Saved goods" : query ? `Results for “${query}”` : category ? category : "All products"}<span aria-label={`${visible.length} products`}>{visible.length}</span></h1></div><label className="ob-sort">Sort by <select aria-label="Sort goods" value={sort} onChange={e => setSort(e.target.value)}><option value="name">Name</option><option value="newest">Newest first</option><option value="price-asc">Price: low to high</option><option value="price-desc">Price: high to low</option></select></label></div>
              <div className="ob-filter-row" role="group" aria-label="Filter by category"><button className={`ob-filter ${!category ? "is-active" : ""}`} aria-pressed={!category} onClick={() => setCategory(null)}><Icon name="grid" size={15}/>All goods</button>{categories.map(c => <button key={c} className={`ob-filter ${category === c ? "is-active" : ""}`} aria-pressed={category === c} onClick={() => setCategory(c)}><Icon name={categoryIcons[c] ?? "grid"} size={15}/>{c}</button>)}</div>
              <details className="ob-advanced-filters"><summary>Filters <span>{Object.values(filters).filter(Boolean).length ? `${Object.values(filters).filter(Boolean).length} active` : "Seller, price & more"}</span><Icon name="chevron" size={13}/></summary><div className="ob-advanced-filter-grid"><label className="ob-field">Seller<select aria-label="Filter by seller" value={filters.seller} onChange={e => setFilters(f => ({ ...f, seller: e.target.value }))}><option value="">All sellers</option>{[...new Set(catalog.listings.map(l => l.seller))].map(s => <option key={s}>{s}</option>)}</select></label><label className="ob-field">Type of good<select aria-label="Filter by type" value={filters.kind} onChange={e => setFilters(f => ({ ...f, kind: e.target.value }))}><option value="">All goods</option><option value="physical">Physical goods</option><option value="digital">Digital goods</option></select></label><label className="ob-field">Ships from<select aria-label="Filter by origin" value={filters.shipFrom} onChange={e => setFilters(f => ({ ...f, shipFrom: e.target.value }))}><option value="">Anywhere</option>{[...new Set(catalog.listings.map(l => l.shipFrom))].map(s => <option key={s}>{s}</option>)}</select></label><label className="ob-field">Min. price in sats<input type="number" min="0" inputMode="numeric" value={filters.min} placeholder="No minimum" onChange={e => setFilters(f => ({ ...f, min: e.target.value }))}/></label><label className="ob-field">Max. price in sats<input type="number" min="0" inputMode="numeric" value={filters.max} placeholder="No maximum" onChange={e => setFilters(f => ({ ...f, max: e.target.value }))}/></label><button className="ob-text-link" onClick={() => setFilters({ seller: "", kind: "", shipFrom: "", min: "", max: "" })}>Reset filters</button></div></details>
              {visible.length ? <div className="ob-product-grid">{visible.map(listing => { const verdict = verdictOf(listing); return verdict.action === "cover" && !revealed.includes(listing.hash) ? <CoveredCard key={listing.hash} reason={verdict.reason} onShow={() => setRevealed(r => [...r, listing.hash])}/> : <ProductCard key={listing.hash} listing={listing} {...money} saved={saved.includes(listing.hash)} onSave={() => toggleSaved(listing)} onOpen={() => open(listing)}/>; })}</div> : <Empty illustration={section === "saved" && saved.length === 0 && !query && !category && !Object.values(filters).some(Boolean) ? "saved" : undefined} icon={section === "saved" ? "heart" : "search"} title={section === "saved" && !query && !category ? "No saved products" : "No matching products"} text={section === "saved" && !query && !category ? "Save products using the heart button." : "Try a different search or explore another category."}><button className="ob-secondary" onClick={() => { setQuery(""); setCategory(null); setFilters({ seller: "", kind: "", shipFrom: "", min: "", max: "" }); if (section === "saved") navigate("discover"); }}>Show all products <Icon name="arrow" size={16}/></button></Empty>}

            </section>
          </>)}

        </div>
      </main>
    </div>
    {toast && <div className="ob-toast" role="status"><Icon name="check" size={17}/>{toast}</div>}
  </div>}</NodePaymentsProvider>;
}
function NavItem({ icon, children, active, count, onClick }: { icon: IconName; children: ReactNode; active: boolean; count?: number; onClick: () => void }) { return <button className={`ob-nav-item ${active ? "is-active" : ""}`} aria-label={typeof children === "string" ? children : undefined} title={typeof children === "string" ? children : undefined} aria-current={active ? "page" : undefined} onClick={onClick}><Icon name={icon} size={19}/><span>{children}</span>{count !== undefined && <span className="ob-nav-count">{count}</span>}</button>; }
export function Empty({ icon, illustration, title, text, children }: { icon: IconName; illustration?: "bag" | "saved"; title: string; text: string; children?: ReactNode }) { return <div className="ob-empty">{illustration ? <MarketEmptyArtwork variant={illustration}/> : <span className="ob-empty-icon"><Icon name={icon} size={30}/></span>}<h2>{title}</h2><p>{text}</p>{children && <div className="ob-empty-actions">{children}</div>}</div>; }
export function ProductImage({ listing, image = listing.images[0], ...props }: { listing: ListingHeader; image?: string; className?: string }) { const src = photo(listing); const placeholder = <Icon name={categoryIcons[listing.category] ?? "bag"} size={55}/>; return <div className={`ob-product-image ${src || image ? "" : "ob-product-placeholder"} ${props.className ?? ""}`}>{image ? <MediaImage hash={image} alt={listing.title} fallback={placeholder}/> : src ? <img src={src} alt={listing.title} loading="lazy"/> : placeholder}</div>; }
export function ProductCard({ listing, rates, currency, saved, onSave, onOpen }: { listing: ListingHeader; rates: RateTable; currency: CurrencyCode; saved: boolean; onSave?: () => void; onOpen: () => void }) {
  return <article className="ob-product-card"><div className="ob-card-image-wrap"><button className="ob-product-open" onClick={onOpen} aria-label={`View ${listing.title}`}><ProductImage listing={listing}/></button>{onSave && <button className={`ob-save ${saved ? "is-saved" : ""}`} aria-label={`${saved ? "Unsave" : "Save"} ${listing.title}`} aria-pressed={saved} onClick={onSave}><Icon name="heart" size={17}/></button>}{listing.kind === "digital" && <span className="ob-digital-badge"><Icon name="bolt" size={12}/>Digital</span>}</div><div className="ob-card-category">{listing.category}<span>{listing.shipFrom}</span></div><button className="ob-product-title" onClick={onOpen}>{listing.title}</button>{listing.price && <span className="ob-seller-price">{formatPrice(listing.price.amount, listing.price.currency)}</span>}<Money sats={listing.priceSats} rates={rates} currency={currency}/><div className="ob-card-seller"><span className="ob-seller-avatar">{listing.seller.charAt(0)}</span>{listing.seller}</div></article>;
}
function ProductDetail({ listing, own, onEdit, rates, currency, saved, onSave, onBack, onBuy, onAdd, alternatives, onAlternative }: { own: boolean; onEdit: () => void; alternatives: ListingHeader[]; onAlternative: (listing: ListingHeader) => void; listing: ListingHeader; rates: RateTable; currency: CurrencyCode; saved: boolean; onSave: () => void; onBack: () => void; onBuy: (pick: BuyChoice) => void; onAdd: (pick: BuyChoice) => void }) {
  const [alternate, setAlternate] = useState(false);
  const buy = useBuyChoice(listing);
  const [shown, setShown] = useState(listing.images[0]);
  return <section className="ob-detail"><button className="ob-back" onClick={onBack}><Icon name="arrow" size={16}/>Back to products</button><div className="ob-detail-grid"><div className="ob-gallery">{listing.images.length ? <><ProductImage listing={listing} image={shown}/>{listing.images.length > 1 && <div className="ob-thumbnails">{listing.images.map((hash, i) => <button key={hash} aria-label={`Show photo ${i + 1}`} aria-pressed={shown === hash} onClick={() => setShown(hash)}><MediaImage hash={hash} alt={`Photo ${i + 1}`}/></button>)}</div>}</> : alternate ? <div className="ob-product-image"><img src="goods/paperback-open.jpg" alt="Paperback opened to its pages"/></div> : <ProductImage listing={listing}/ >}{listing.title === "Paperback novel" && <div className="ob-thumbnails"><button aria-label="Show book cover" aria-pressed={!alternate} onClick={() => setAlternate(false)}><img src="goods/paperback.jpg" alt="Book cover"/></button><button aria-label="Show inside pages" aria-pressed={alternate} onClick={() => setAlternate(true)}><img src="goods/paperback-open.jpg" alt="Inside pages"/></button></div>}</div><div className="ob-detail-copy"><span className="ob-eyebrow">{listing.category} · {listing.kind === "digital" ? "Digital" : "Physical item"}</span><h1>{listing.title}</h1><div className="ob-detail-seller"><span className="ob-seller-avatar">{listing.seller.charAt(0)}</span><span>Sold by <strong>{listing.seller}</strong></span></div>{listing.commerce ? <ListingBuyTerms listing={listing} buy={buy}/> : <><Money sats={listing.priceSats} rates={rates} currency={currency} large/><p className="ob-btc-price">{btc(listing.priceSats)}{listing.price && ` · Seller's price ${formatPrice(listing.price.amount, listing.price.currency)}${listing.price.shipping ? ` + ${formatPrice(listing.price.shipping, listing.price.currency)} shipping` : ""}, paid in sats at the rate when you order`}</p></>}<p className="ob-description">{listing.description || "No description provided."}</p><div className="ob-product-facts"><span><Icon name={listing.kind === "digital" ? "bolt" : "pin"} size={18}/>{listing.kind === "digital" ? "Delivered digitally" : `Ships from ${listing.shipFrom}`}</span>{!listing.commerce && <span><span className={`ob-status-dot ${listing.stock <= 0 ? "is-offline" : ""}`}/>{listing.stock > 0 ? `${listing.stock} available` : "Currently out of stock"}</span>}</div><ListingBuyFacts listing={listing}/>{own ? <div className="ob-detail-actions"><p className="ob-page-intro">This is your listing.</p><button className="ob-secondary" onClick={onEdit}>Edit in your listings <Icon name="arrow" size={18}/></button></div> : <div className="ob-detail-actions"><button className="ob-primary" disabled={!buy.canBuy} onClick={() => onAdd(buy.choice)}>Add to bag <Icon name="bag" size={18}/></button><button className="ob-secondary" disabled={!buy.canBuy} onClick={() => onBuy(buy.choice)}>Buy now <Icon name="arrow" size={18}/></button><button className={`ob-secondary ob-save-detail ${saved ? "is-saved" : ""}`} aria-pressed={saved} onClick={onSave}><Icon name="heart" size={18}/>{saved ? "Saved" : "Save for later"}</button></div>}<div className="ob-payment-note"><Icon name="shield" size={22}/><div><strong>Payment options</strong><p>{listing.acceptsModerator ? "Pay the seller directly, or choose moderated escrow at checkout." : "Pay the seller directly with bitcoin."}</p></div></div>{alternatives.length > 0 && <div className="ob-other-sellers"><h2>Other sellers</h2>{alternatives.map(other => <button key={other.hash} onClick={() => onAlternative(other)}><span><strong>{other.seller}</strong><small>Ships from {other.shipFrom}</small></span><Money sats={other.priceSats} rates={rates} currency={currency}/><Icon name="arrow" size={16}/></button>)}</div>}</div></div></section>;
}
export function Payment({ receipt }: { receipt: BuyerOrder | import("../catalog").SellerOrder }) {
  const id = "id" in receipt ? receipt.id : receipt.orderId;
  if (receipt.payment?.ln) return <LightningOrderPayment key={id} id={id} initial={receipt} invoice={receipt.payment.ln}/>;
  return <OrderPayment key={id} id={id} initial={receipt}/>;
}
function Orders({ rates, currency, onBrowse }: { rates: RateTable; currency: CurrencyCode; onBrowse: () => void }) {
  const resource = useNodeResource(fetchOrders, 12000, true, orders => orders.length === 0 || orders.every(o => o.remote ? o.status === "Completed" || o.status === "Declined by the seller" : o.status === "Paid"));
  const [expanded, setExpanded] = useState<string | null>(null);
  const orders = resource.data ?? [];
  return <section className="ob-orders"><PageHeading title="Your orders" description="Track deliveries and complete payments." action={<button className="ob-secondary" disabled={resource.loading} onClick={() => void resource.refresh()}>Refresh orders</button>}/>
    {resource.error && <p className="ob-form-error" role="alert">{resource.error}</p>}
    {resource.loading ? <p role="status">Loading orders…</p> : !orders.length ? <Empty icon="bag" title="No orders yet" text="Orders appear here after checkout."><button className="ob-primary" onClick={onBrowse}>Browse products</button></Empty> : orders.map(order => <article className="ob-order-row" key={order.id}>
      <button className="ob-order-toggle" onClick={() => setExpanded(expanded === order.id ? null : order.id)} aria-expanded={expanded === order.id} aria-label={`Order from ${order.seller}`}><span className="ob-order-icon"><Icon name="bag"/></span><span className="ob-order-name"><strong>{order.lines.map(l => l.title).join(", ")}</strong><span>{order.seller} · {new Date(order.created * (order.created < 1e12 ? 1000 : 1)).toLocaleDateString()}</span></span><span className="ob-order-status">{order.status}</span><Money sats={order.totalSats} rates={rates} currency={currency}/><Icon name="chevron" size={16}/></button>
      {expanded === order.id && <div className="ob-order-expanded"><p className="ob-meta">Order {order.id}</p><RemoteOrderStatus order={order} onChange={() => void resource.refresh()}/>{(!order.remote || order.payment) && <Payment receipt={order}/>}</div>}
    </article>)}
  </section>;
}
