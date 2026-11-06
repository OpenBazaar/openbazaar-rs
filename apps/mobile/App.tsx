import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Image,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  useWindowDimensions,
  View,
  type PressableStateCallbackType,
  type TextStyle,
  type ViewStyle,
} from "react-native";

import {
  createAddress,
  deleteAddress,
  fetchAddresses,
  fetchCatalog,
  fetchLabelers,
  fetchOrder,
  fetchOrders,
  fetchRates,
  NodeOfflineError,
  OFFLINE_MESSAGE,
  placeCheckout,
  setLabelers,
  updateAddress,
  type AddressInput,
  type LabelerState,
  type BuyerOrder,
  type CheckoutReceipt,
  type CurrencyCode,
  type ListingHeader,
  type Moderator,
  type RateTable,
  type SavedAddress,
  type ShopCatalog,
} from "./src/catalog";
import { alsoSoldBy, cartPackages, type CartLine } from "./src/cart";
import { photosFor } from "./src/goods";
import { filterListings } from "./src/search";
import { CONTENT_TAGS, parseKeywords, safetyVerdict, toggleHidden, visibleListings, type SafetySettings } from "./src/safety/filter";
import { readSafety, writeSafety } from "./src/safety/storage";

type SortKey = "relevance" | "price-asc" | "price-desc" | "newest";
type Filters = {
  category: string | null;
  seller: string | null;
  kind: ListingHeader["kind"] | null;
  shipFrom: string | null;
  min: string;
  max: string;
};
type Route =
  | { name: "shop" }
  | { name: "detail"; hash: string }
  | { name: "cart" }
  | { name: "saved" }
  | { name: "addresses" }
  | { name: "orders" }
  | { name: "order"; id: string }
  | { name: "checkout" }
  | { name: "done"; receipt: CheckoutReceipt }
  | { name: "safety" };

const NAVY = "#063853";
const BLUE = "#0e6da5";
const INK = "#242424";
const SECONDARY = "#4a4a4a";
const MUTED = "#6b6b6b";
const LINE = "rgba(6, 56, 83, 0.12)";
const PAGE = "#f5f7f8";
const CARD = "#ffffff";
const SOFT = "#e7eef2";
const ERROR = "#9f2f2f";
const SANS = "Instrument Sans";
const CURRENCIES: readonly CurrencyCode[] = ["USD", "EUR", "GBP", "JPY"];
const EMPTY_FILTERS: Filters = { category: null, seller: null, kind: null, shipFrom: null, min: "", max: "" };
const ADDRESS_NOTE =
  "This address is shared only with the seller you buy from, and a pickup point can be the label if you would rather not use a home address.";

function formatNumber(value: number): string {
  return value.toLocaleString("en-US");
}

function formatBtc(sats: number): string {
  const fixed = (sats / 100_000_000).toFixed(8);
  const trimmed = fixed.replace(/0+$/, "").replace(/\.$/, "");
  const [whole, frac = ""] = trimmed.split(".");
  return `${whole}.${frac.padEnd(4, "0")} BTC`;
}

function formatFiat(sats: number, rates: RateTable, currency: CurrencyCode): string | null {
  const rate = rates[currency];
  if (typeof rate !== "number") return null;
  const amount = (sats / 100_000_000) * rate;
  if (!Number.isFinite(amount)) return null;
  if (currency === "JPY") return `¥${Math.round(amount).toLocaleString("en-US")}`;
  const symbol = currency === "EUR" ? "€" : currency === "GBP" ? "£" : "$";
  return `${symbol}${amount.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

function priceLabel(sats: number, rates: RateTable, currency: CurrencyCode): string {
  const fiat = formatFiat(sats, rates, currency);
  return fiat ? `${formatBtc(sats)} (${fiat})` : formatBtc(sats);
}

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // The page still keeps the value for this visit.
  }
}

function readCurrency(): CurrencyCode {
  if (typeof localStorage === "undefined") return "USD";
  const saved = localStorage.getItem("ob-shop-currency");
  return CURRENCIES.includes(saved as CurrencyCode) ? (saved as CurrencyCode) : "USD";
}

function blankAddress(): AddressInput {
  return { label: "", name: "", street: "", street2: "", city: "", region: "", postalCode: "", country: "", default: false };
}

function addressReady(input: AddressInput): boolean {
  return [input.label, input.name, input.street, input.city, input.region, input.postalCode, input.country].every(
    (part) => part.trim().length > 0,
  );
}

export default function App() {
  const { width } = useWindowDimensions();
  const compact = width < 860;
  const [catalog, setCatalog] = useState<ShopCatalog | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [rates, setRates] = useState<RateTable>({});
  const [currency, setCurrency] = useState<CurrencyCode>(readCurrency);
  const [query, setQuery] = useState("");
  const [filters, setFilters] = useState<Filters>(EMPTY_FILTERS);
  const [sort, setSort] = useState<SortKey>("relevance");
  const [view, setView] = useState<"grid" | "list">(
    () => (typeof localStorage !== "undefined" && localStorage.getItem("ob-shop-view") === "list" ? "list" : "grid"),
  );
  const [cart, setCart] = useState<CartLine[]>(() =>
    typeof localStorage === "undefined" ? [] : readJson<CartLine[]>("ob-shop-cart", []),
  );
  const [saved, setSaved] = useState<string[]>(() =>
    typeof localStorage === "undefined" ? [] : readJson<string[]>("ob-shop-saved", []),
  );
  const [route, setRoute] = useState<Route>({ name: "shop" });
  const [menu, setMenu] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [safety, setSafety] = useState<SafetySettings>(readSafety);
  const [revealed, setRevealed] = useState<string[]>([]);
  const [labelers, setLabelerState] = useState<LabelerState | null>(null);
  useEffect(() => {
    void fetchLabelers().then(setLabelerState, () => setLabelerState(null));
  }, []);
  const labels = useMemo(() => labelers?.labels ?? [], [labelers]);
  const shown = useMemo(
    () => (catalog ? { ...catalog, listings: visibleListings(catalog.listings, catalog.store.contentRating, safety, labels) } : null),
    [catalog, safety, labels],
  );

  const rememberSafety = (next: SafetySettings) => {
    setSafety(next);
    writeSafety(next);
  };

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      setCatalog(await fetchCatalog());
    } catch (err) {
      setCatalog(null);
      setLoadError(err instanceof NodeOfflineError ? OFFLINE_MESSAGE : err instanceof Error ? err.message : OFFLINE_MESSAGE);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!catalog) return;
    let stop = false;
    const pull = () => {
      void fetchRates().then((next) => {
        if (!stop) setRates(next);
      });
    };
    pull();
    const timer = setInterval(pull, 5 * 60 * 1000);
    return () => {
      stop = true;
      clearInterval(timer);
    };
  }, [catalog]);

  useEffect(() => {
    if (Platform.OS !== "web" || typeof document === "undefined") return;
    if (!document.getElementById("ob-shop-fonts")) {
      const link = document.createElement("link");
      link.id = "ob-shop-fonts";
      link.rel = "stylesheet";
      link.href = "fonts/shop.css";
      document.head.appendChild(link);
    }
    document.body.style.margin = "0";
    document.body.style.backgroundColor = PAGE;
    document.title = "OpenBazaar";
  }, []);

  const onCurrency = (next: CurrencyCode) => {
    setCurrency(next);
    try {
      localStorage.setItem("ob-shop-currency", next);
    } catch {
      // kept in the page
    }
  };

  const rememberCart = (next: CartLine[]) => {
    setCart(next);
    writeJson("ob-shop-cart", next);
  };

  const rememberSaved = (next: string[]) => {
    setSaved(next);
    writeJson("ob-shop-saved", next);
  };

  const listingByHash = useCallback(
    (hash: string) => catalog?.listings.find((listing) => listing.hash === hash),
    [catalog],
  );

  const addToCart = (hash: string, quantity: number) => {
    const next = cart.some((line) => line.hash === hash)
      ? cart.map((line) => (line.hash === hash ? { ...line, quantity: Math.min(99, line.quantity + quantity) } : line))
      : [...cart, { hash, quantity }];
    rememberCart(next);
  };

  const goShop = () => {
    setMenu(false);
    setRoute({ name: "shop" });
  };

  let body: ReactNode = null;
  if (loading) {
    body = <Text style={styles.status}>Loading the shop</Text>;
  } else if (loadError || !catalog) {
    body = (
      <View style={styles.offline}>
        <Image source={{ uri: "brand/ob-mark.png" }} style={styles.offlineMark} accessibilityLabel="OpenBazaar" />
        <Text style={styles.offlineTitle}>Offline</Text>
        <Text style={styles.offlineBody}>{loadError ?? OFFLINE_MESSAGE}</Text>
        <Button label="Try again" onPress={() => void load()} />
      </View>
    );
  } else if (route.name === "shop" || route.name === "saved") {
    body = (
      <Shop
        catalog={shown ?? catalog}
        query={query}
        filters={filters}
        sort={sort}
        view={view}
        savedOnly={route.name === "saved"}
        saved={saved}
        rates={rates}
        currency={currency}
        compact={compact}
        filtersOpen={filtersOpen}
        onFilters={setFilters}
        onSort={setSort}
        onView={(next) => {
          setView(next);
          try {
            localStorage.setItem("ob-shop-view", next);
          } catch {
            // kept in the page
          }
        }}
        onOpenFilters={setFiltersOpen}
        onOpen={(hash) => setRoute({ name: "detail", hash })}
        onToggleSave={(hash) => rememberSaved(saved.includes(hash) ? saved.filter((item) => item !== hash) : [...saved, hash])}
        onClearQuery={() => setQuery("")}
        coverReason={(listing) => {
          if (revealed.includes(listing.hash)) return null;
          const verdict = safetyVerdict(listing, catalog.store.contentRating, safety, labels);
          return verdict.action === "cover" ? verdict.reason : null;
        }}
        onReveal={(hash) => setRevealed([...revealed, hash])}
      />
    );
  } else if (route.name === "detail") {
    const listing = listingByHash(route.hash);
    const verdict = listing ? safetyVerdict(listing, catalog.store.contentRating, safety, labels) : null;
    body = listing && verdict && verdict.action !== "show" && !revealed.includes(listing.hash) ? (
      <View style={styles.sheetPad}>
        <Text style={styles.pageTitle}>This listing is hidden on this device.</Text>
        <Text style={styles.body}>{verdict.reason} You can change this in Content filters.</Text>
        <Button label="Show it anyway" onPress={() => setRevealed([...revealed, listing.hash])} />
        <TextButton label="Back to the shop" onPress={goShop} />
      </View>
    ) : listing ? (
      <Detail
        listing={listing}
        rates={rates}
        currency={currency}
        saved={saved.includes(listing.hash)}
        onToggleSave={() =>
          rememberSaved(saved.includes(listing.hash) ? saved.filter((item) => item !== listing.hash) : [...saved, listing.hash])
        }
        onAdd={(quantity) => addToCart(listing.hash, quantity)}
        onBuy={(quantity) => {
          addToCart(listing.hash, quantity);
          setRoute({ name: "checkout" });
        }}
        onHide={(kind) => {
          rememberSafety(
            kind === "listing"
              ? { ...safety, hiddenListings: toggleHidden(safety.hiddenListings, { id: listing.hash, label: listing.title }) }
              : { ...safety, hiddenStores: toggleHidden(safety.hiddenStores, { id: listing.storeId, label: listing.seller }) },
          );
          goShop();
        }}
      />
    ) : (
      <Text style={styles.status}>That listing is not in this shop.</Text>
    );
  } else if (route.name === "cart") {
    body = (
      <Cart
        catalog={catalog}
        lines={cart}
        rates={rates}
        currency={currency}
        onChange={(hash, quantity) =>
          rememberCart(quantity < 1 ? cart.filter((line) => line.hash !== hash) : cart.map((line) => (line.hash === hash ? { ...line, quantity } : line)))
        }
        onSave={(hash) => {
          rememberSaved(saved.includes(hash) ? saved : [...saved, hash]);
          rememberCart(cart.filter((line) => line.hash !== hash));
        }}
        onRemovePackage={(seller) => rememberCart(cart.filter((line) => listingByHash(line.hash)?.seller !== seller))}
        onCheckout={() => setRoute({ name: "checkout" })}
      />
    );
  } else if (route.name === "checkout") {
    body = (
      <Checkout
        catalog={catalog}
        lines={cart}
        rates={rates}
        currency={currency}
        onOffline={() => {
          setCatalog(null);
          setLoadError(OFFLINE_MESSAGE);
          setRoute({ name: "shop" });
        }}
        onDone={(receipt, bought) => {
          rememberCart(cart.filter((line) => !bought.includes(line.hash)));
          setRoute({ name: "done", receipt });
        }}
      />
    );
  } else if (route.name === "done") {
    body = <Confirmation receipt={route.receipt} rates={rates} currency={currency} onOrders={() => setRoute({ name: "orders" })} onShop={goShop} />;
  } else if (route.name === "safety") {
    body = <SafetyScreen settings={safety} onChange={rememberSafety} labelers={labelers} onLabelers={setLabelerState} />;
  } else if (route.name === "addresses") {
    body = <Addresses onOffline={() => { setCatalog(null); setLoadError(OFFLINE_MESSAGE); setRoute({ name: "shop" }); }} />;
  } else if (route.name === "orders") {
    body = (
      <Orders
        rates={rates}
        currency={currency}
        onOpen={(id) => setRoute({ name: "order", id })}
        onOffline={() => { setCatalog(null); setLoadError(OFFLINE_MESSAGE); setRoute({ name: "shop" }); }}
      />
    );
  } else {
    body = (
      <OrderDetail
        id={route.id}
        rates={rates}
        currency={currency}
        onOffline={() => { setCatalog(null); setLoadError(OFFLINE_MESSAGE); setRoute({ name: "shop" }); }}
      />
    );
  }

  const cartCount = cart.reduce((sum, line) => sum + line.quantity, 0);

  return (
    <View style={styles.page}>
      <View style={styles.trust}>
        <Text style={styles.trustText}>Orders are sealed on this machine</Text>
        <Text style={styles.trustText}>Escrow when the listing accepts it</Text>
        <Text style={styles.trustText}>You pay in bitcoin</Text>
      </View>
      {catalog && !loadError ? (
        <View style={styles.header}>
          <Pressable accessibilityRole="button" accessibilityLabel="OpenBazaar home" onPress={goShop} style={styles.brand}>
            <Image source={{ uri: "brand/ob-mark.png" }} style={styles.mark} accessibilityLabel="OpenBazaar" />
            <Text style={styles.wordmark}>OpenBazaar</Text>
          </Pressable>
          <SearchField
            query={query}
            onQuery={(value) => {
              setQuery(value);
              setRoute({ name: "shop" });
            }}
          />
          <View style={styles.headerTools}>
            <View style={styles.currencies}>
              {CURRENCIES.map((code) => (
                <Pressable key={code} accessibilityRole="button" accessibilityLabel={`Show prices in ${code}`} onPress={() => onCurrency(code)} style={[styles.currency, currency === code && styles.currencyOn]}>
                  <Text style={[styles.currencyText, currency === code && styles.currencyTextOn]}>{code}</Text>
                </Pressable>
              ))}
            </View>
            <IconCount label="Saved" icon={saved.length ? "icons/heart-on.svg" : "icons/heart.svg"} count={saved.length} onPress={() => { setMenu(false); setRoute({ name: "saved" }); }} />
            <IconCount label="Cart" icon="icons/cart.svg" count={cartCount} onPress={() => { setMenu(false); setRoute({ name: "cart" }); }} />
            <View>
              <Pressable accessibilityRole="button" accessibilityLabel="You" onPress={() => setMenu((open) => !open)} style={styles.you}>
                <Text style={styles.youText}>You</Text>
              </Pressable>
              {menu ? (
                <View style={styles.menu}>
                  <TextButton label="Addresses" onPress={() => { setMenu(false); setRoute({ name: "addresses" }); }} />
                  <TextButton label="Orders" onPress={() => { setMenu(false); setRoute({ name: "orders" }); }} />
                  <TextButton label="Content filters" onPress={() => { setMenu(false); setRoute({ name: "safety" }); }} />
                </View>
              ) : null}
            </View>
          </View>
        </View>
      ) : null}
      <View style={styles.fill}>{body}</View>
    </View>
  );
}

function Shop({
  catalog,
  query,
  filters,
  sort,
  view,
  savedOnly,
  saved,
  rates,
  currency,
  compact,
  filtersOpen,
  onFilters,
  onSort,
  onView,
  onOpenFilters,
  onOpen,
  onToggleSave,
  onClearQuery,
  coverReason,
  onReveal,
}: {
  catalog: ShopCatalog;
  query: string;
  filters: Filters;
  sort: SortKey;
  view: "grid" | "list";
  savedOnly: boolean;
  saved: string[];
  rates: RateTable;
  currency: CurrencyCode;
  compact: boolean;
  filtersOpen: boolean;
  onFilters: (filters: Filters) => void;
  onSort: (sort: SortKey) => void;
  onView: (view: "grid" | "list") => void;
  onOpenFilters: (open: boolean) => void;
  onOpen: (hash: string) => void;
  onToggleSave: (hash: string) => void;
  onClearQuery: () => void;
  coverReason: (listing: ListingHeader) => string | null;
  onReveal: (hash: string) => void;
}) {
  const { width } = useWindowDimensions();
  const columns = width >= 1280 ? 3 : width >= 900 ? 2 : 1;
  const visible = useMemo(() => {
    let listings = savedOnly ? catalog.listings.filter((listing) => saved.includes(listing.hash)) : catalog.listings;
    if (filters.category) listings = listings.filter((listing) => listing.category === filters.category);
    if (filters.seller) listings = listings.filter((listing) => listing.seller === filters.seller);
    if (filters.kind) listings = listings.filter((listing) => listing.kind === filters.kind);
    if (filters.shipFrom) listings = listings.filter((listing) => listing.shipFrom === filters.shipFrom);
    const min = Number(filters.min);
    const max = Number(filters.max);
    const rate = rates[currency];
    const amount = (sats: number) => (typeof rate === "number" ? (sats / 1e8) * rate : sats);
    if (filters.min.trim() && Number.isFinite(min)) listings = listings.filter((listing) => amount(listing.priceSats) >= min);
    if (filters.max.trim() && Number.isFinite(max)) listings = listings.filter((listing) => amount(listing.priceSats) <= max);
    listings = filterListings(listings, query);
    const sorted = [...listings];
    if (sort === "price-asc") sorted.sort((a, b) => a.priceSats - b.priceSats);
    if (sort === "price-desc") sorted.sort((a, b) => b.priceSats - a.priceSats);
    if (sort === "newest") sorted.sort((a, b) => b.listedOn - a.listedOn);
    return sorted;
  }, [catalog.listings, filters, query, rates, currency, saved, savedOnly, sort]);
  const featured = catalog.listings.find((listing) => listing.title === "Paperback novel") ?? catalog.listings[0];
  const browsing = !savedOnly && !query.trim() && !filters.category && !filters.seller && !filters.kind && !filters.shipFrom && !filters.min && !filters.max;
  const crumb = savedOnly ? "Saved" : filters.category ?? (query.trim() ? "Search" : "All goods");
  const count = query.trim() ? `${formatNumber(visible.length)} results for ${query.trim()}` : `${formatNumber(visible.length)} results`;
  const rail = (
    <FilterRail
      catalog={catalog}
      filters={filters}
      currency={currency}
      hasRate={typeof rates[currency] === "number"}
      onFilters={onFilters}
    />
  );

  return (
    <View style={styles.browse}>
      {compact ? null : rail}
      {compact && filtersOpen ? (
        <View style={styles.filterSheet}>
          <View style={styles.filterSheetHead}>
            <Text style={styles.section}>Filters</Text>
            <TextButton label="Close" onPress={() => onOpenFilters(false)} />
          </View>
          {rail}
        </View>
      ) : null}
      <ScrollView style={styles.fill} contentContainerStyle={styles.results}>
        {browsing && featured ? (
          <Pressable accessibilityRole="button" accessibilityLabel={featured.title} onPress={() => onOpen(featured.hash)} style={[styles.hero, compact && styles.heroCompact]}>
            <Image source={{ uri: photosFor(featured.title, featured.category)[0] }} style={[styles.heroImage, compact && styles.heroImageCompact]} />
            <View style={styles.heroCopy}>
              <Text style={styles.kicker}>{featured.category}</Text>
              <Text style={[styles.heroTitle, compact && styles.heroTitleCompact]}>{featured.title}</Text>
              <Text style={styles.heroSeller}>{featured.seller}</Text>
              <Text style={styles.heroPrice}>{priceLabel(featured.priceSats, rates, currency)}</Text>
            </View>
          </Pressable>
        ) : null}
        <Text style={styles.crumb}>Home / {crumb}</Text>
        <View style={styles.resultBar}>
          <Text style={styles.resultCount}>{count}</Text>
          <View style={styles.resultTools}>
            {compact ? <TextButton label="Filter" onPress={() => onOpenFilters(true)} /> : null}
            <TextButton label={view === "grid" ? "List" : "Grid"} onPress={() => onView(view === "grid" ? "list" : "grid")} />
            {(["relevance", "price-asc", "price-desc", "newest"] as const).map((key) => (
              <Pressable key={key} accessibilityRole="button" onPress={() => onSort(key)} style={[styles.sort, sort === key && styles.sortOn]}>
                <Text style={[styles.sortText, sort === key && styles.sortTextOn]}>
                  {key === "relevance" ? "Relevance" : key === "price-asc" ? "Price low" : key === "price-desc" ? "Price high" : "Newest"}
                </Text>
              </Pressable>
            ))}
          </View>
        </View>
        {visible.length === 0 ? (
          <View>
            <Text style={styles.emptyTitle}>{query.trim() ? `Nothing matches “${query.trim()}”` : "Nothing in this view"}</Text>
            <TextButton label="Clear search" onPress={onClearQuery} />
            <TextButton label="Clear filters" onPress={() => onFilters(EMPTY_FILTERS)} />
          </View>
        ) : (
          <View style={view === "grid" ? styles.grid : styles.list}>
            {visible.map((listing) => coverReason(listing) ? (
              <View key={listing.hash} style={[styles.card, styles.cardBody]}>
                <Text style={styles.meta}>Covered by your content filters. {coverReason(listing)}</Text>
                <TextButton label="Show" onPress={() => onReveal(listing.hash)} />
              </View>
            ) : (
              <ListingCard
                key={listing.hash}
                listing={listing}
                rates={rates}
                currency={currency}
                saved={saved.includes(listing.hash)}
                wide={view === "list" || columns === 1}
                columns={view === "list" ? 1 : columns}
                onOpen={() => onOpen(listing.hash)}
                onSave={() => onToggleSave(listing.hash)}
              />
            ))}
          </View>
        )}
      </ScrollView>
    </View>
  );
}

function FilterRail({
  catalog,
  filters,
  currency,
  hasRate,
  onFilters,
}: {
  catalog: ShopCatalog;
  filters: Filters;
  currency: CurrencyCode;
  hasRate: boolean;
  onFilters: (filters: Filters) => void;
}) {
  const categories = unique(catalog.listings.map((listing) => listing.category));
  const sellers = unique(catalog.listings.map((listing) => listing.seller));
  const regions = unique(catalog.listings.map((listing) => listing.shipFrom));
  return (
    <ScrollView style={styles.rail} contentContainerStyle={styles.railBody}>
      <FilterGroup label="Category" value={filters.category} options={categories} onChange={(category) => onFilters({ ...filters, category })} />
      <FilterGroup label="Seller" value={filters.seller} options={sellers} onChange={(seller) => onFilters({ ...filters, seller })} />
      <FilterGroup label="Kind" value={filters.kind} options={["physical", "digital"]} onChange={(kind) => onFilters({ ...filters, kind: kind as Filters["kind"] })} />
      <FilterGroup label="Ship from" value={filters.shipFrom} options={regions} onChange={(shipFrom) => onFilters({ ...filters, shipFrom })} />
      <Text style={styles.filterLabel}>Price ({hasRate ? currency : "sats"})</Text>
      <View style={styles.priceRow}>
        <TextInput value={filters.min} onChangeText={(min) => onFilters({ ...filters, min })} placeholder="Min" placeholderTextColor={MUTED} keyboardType="decimal-pad" style={styles.priceInput} accessibilityLabel="Minimum price" />
        <TextInput value={filters.max} onChangeText={(max) => onFilters({ ...filters, max })} placeholder="Max" placeholderTextColor={MUTED} keyboardType="decimal-pad" style={styles.priceInput} accessibilityLabel="Maximum price" />
      </View>
    </ScrollView>
  );
}

function FilterGroup({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string | null;
  options: string[];
  onChange: (value: string | null) => void;
}) {
  return (
    <View style={styles.filterGroup}>
      <Text style={styles.filterLabel}>{label}</Text>
      <FilterRow label="All" selected={value === null} onPress={() => onChange(null)} />
      {options.map((option) => (
        <FilterRow key={option} label={option === "physical" ? "Physical" : option === "digital" ? "Digital" : option} selected={value === option} onPress={() => onChange(option)} />
      ))}
    </View>
  );
}

function FilterRow({ label, selected, onPress }: { label: string; selected: boolean; onPress: () => void }) {
  return (
    <Pressable accessibilityRole="button" accessibilityState={{ selected }} onPress={onPress} style={[styles.filterRow, selected && styles.filterRowOn]}>
      <Text style={[styles.filterText, selected && styles.filterTextOn]}>{label}</Text>
    </Pressable>
  );
}

function ListingCard({
  listing,
  rates,
  currency,
  saved,
  wide,
  columns,
  onOpen,
  onSave,
}: {
  listing: ListingHeader;
  rates: RateTable;
  currency: CurrencyCode;
  saved: boolean;
  wide: boolean;
  columns: number;
  onOpen: () => void;
  onSave: () => void;
}) {
  const width = columns <= 1 ? "100%" : (`${100 / columns - 2}%` as ViewStyle["width"]);
  return (
    <View style={[styles.card, wide && styles.cardRow, { width }]}>
      <Pressable accessibilityRole="button" accessibilityLabel={`${listing.title}, ${priceLabel(listing.priceSats, rates, currency)}`} onPress={onOpen} style={wide ? styles.cardImageWide : styles.cardImageWrap}>
        <Image source={{ uri: photosFor(listing.title, listing.category)[0] }} style={styles.cardImage} />
      </Pressable>
      <View style={styles.cardBody}>
        <View style={styles.badgeRow}>
          {listing.kind === "digital" ? <Text style={styles.badge}>Digital</Text> : null}
          {listing.acceptsModerator ? <Text style={styles.badge}>Escrow available</Text> : null}
          {listing.stock > 0 && listing.stock <= 3 ? <Text style={styles.badge}>Low stock</Text> : null}
          <Pressable accessibilityRole="button" accessibilityLabel={saved ? "Remove from saved" : "Save"} onPress={onSave} style={styles.heart}>
            <Image source={{ uri: saved ? "icons/heart-on.svg" : "icons/heart.svg" }} style={styles.heartIcon} />
          </Pressable>
        </View>
        <Text style={styles.seller}>{listing.seller}</Text>
        <Text style={styles.cardTitle}>{listing.title}</Text>
        <Text style={styles.price}>{priceLabel(listing.priceSats, rates, currency)}</Text>
      </View>
    </View>
  );
}

function Detail({
  listing,
  rates,
  currency,
  saved,
  onToggleSave,
  onAdd,
  onBuy,
  onHide,
}: {
  listing: ListingHeader;
  rates: RateTable;
  currency: CurrencyCode;
  saved: boolean;
  onToggleSave: () => void;
  onHide: (kind: "listing" | "seller") => void;
  onAdd: (quantity: number) => void;
  onBuy: (quantity: number) => void;
}) {
  const photos = photosFor(listing.title, listing.category);
  const [photo, setPhoto] = useState(0);
  const [quantityText, setQuantityText] = useState("1");
  const quantity = Number(quantityText);
  const valid = Number.isInteger(quantity) && quantity >= 1 && quantity <= 99;
  return (
    <ScrollView contentContainerStyle={styles.detail}>
      <View style={styles.detailMain}>
        <Image source={{ uri: photos[photo] ?? photos[0] }} style={styles.detailImage} />
        {photos.length > 1 ? (
          <View style={styles.thumbs}>
            {photos.map((uri, index) => (
              <Pressable key={uri} accessibilityRole="button" accessibilityLabel={`Photo ${index + 1}`} onPress={() => setPhoto(index)}>
                <Image source={{ uri }} style={[styles.thumb, photo === index && styles.thumbOn]} />
              </Pressable>
            ))}
          </View>
        ) : null}
        <Text style={styles.kicker}>{listing.category}</Text>
        <Text style={styles.pageTitle}>{listing.title}</Text>
        <Text style={styles.seller}>{listing.seller}</Text>
        <Text style={styles.body}>{listing.description}</Text>
        <Text style={styles.meta}>Ships from {listing.shipFrom}</Text>
      </View>
      <View style={styles.buyBox}>
        <Text style={styles.heroPrice}>{priceLabel(listing.priceSats, rates, currency)}</Text>
        <Field label="Quantity" value={quantityText} onChangeText={setQuantityText} keyboard="number-pad" />
        {!valid ? <Text style={styles.error}>Enter a quantity from 1 to 99.</Text> : null}
        <Button label="Add to cart" onPress={() => valid && onAdd(quantity)} />
        <Button label="Buy now" onPress={() => valid && onBuy(quantity)} />
        <TextButton label={saved ? "Saved" : "Save"} onPress={onToggleSave} />
        <TextButton label="Hide this listing" onPress={() => onHide("listing")} />
        <TextButton label="Hide this seller" onPress={() => onHide("seller")} />
      </View>
    </ScrollView>
  );
}

function shippingLine(pkg: { physical: boolean; shippingSats: number; freeOverSats: number }, rates: RateTable, currency: CurrencyCode): string {
  if (!pkg.physical) return "No shipping";
  if (pkg.shippingSats === 0) return "Free shipping";
  const price = priceLabel(pkg.shippingSats, rates, currency);
  if (pkg.freeOverSats > 0) return `Shipping ${price}. Free shipping from ${priceLabel(pkg.freeOverSats, rates, currency)}.`;
  return `Shipping ${price}`;
}

function Cart({
  catalog,
  lines,
  rates,
  currency,
  onChange,
  onSave,
  onRemovePackage,
  onCheckout,
}: {
  catalog: ShopCatalog;
  lines: CartLine[];
  rates: RateTable;
  currency: CurrencyCode;
  onChange: (hash: string, quantity: number) => void;
  onSave: (hash: string) => void;
  onRemovePackage: (seller: string) => void;
  onCheckout: () => void;
}) {
  const packages = cartPackages(catalog, lines);
  if (packages.length === 0) {
    return <Text style={styles.status}>Your cart is empty.</Text>;
  }
  const itemCount = packages.reduce((sum, pkg) => sum + pkg.items.reduce((count, item) => count + item.quantity, 0), 0);
  const itemsSats = packages.reduce((sum, pkg) => sum + pkg.itemsSats, 0);
  const shippingSats = packages.reduce((sum, pkg) => sum + pkg.shippingSats, 0);
  return (
    <ScrollView contentContainerStyle={styles.detail}>
      <View style={styles.detailMain}>
        <Text style={styles.pageTitle}>Cart</Text>
        {packages.map((pkg) => (
          <View key={pkg.seller} style={styles.group}>
            <View style={styles.packageHead}>
              <View style={styles.cardBody}>
                <Text style={styles.section}>{pkg.seller}</Text>
                <Text style={styles.meta}>Ships from {pkg.shipFrom}</Text>
              </View>
              <TextButton label="Remove package" name={`Remove ${pkg.seller} package`} onPress={() => onRemovePackage(pkg.seller)} />
            </View>
            {pkg.items.map((item) => (
              <View key={item.listing.hash} style={styles.cartLine}>
                <Image source={{ uri: photosFor(item.listing.title, item.listing.category)[0] }} style={styles.cartImage} />
                <View style={styles.cardBody}>
                  <Text style={styles.cardTitle}>{item.listing.title}</Text>
                  <Text style={styles.price}>{priceLabel(item.listing.priceSats, rates, currency)}</Text>
                  <Text style={styles.meta}>Line {priceLabel(item.listing.priceSats * item.quantity, rates, currency)}</Text>
                  <View style={styles.stepper}>
                    <Pressable accessibilityRole="button" accessibilityLabel={`Decrease ${item.listing.title}`} onPress={() => onChange(item.listing.hash, item.quantity - 1)} style={styles.step}>
                      <Text style={styles.stepText}>−</Text>
                    </Pressable>
                    <Text style={styles.stepValue}>{item.quantity}</Text>
                    <Pressable accessibilityRole="button" accessibilityLabel={`Increase ${item.listing.title}`} onPress={() => onChange(item.listing.hash, Math.min(99, item.quantity + 1))} style={styles.step}>
                      <Text style={styles.stepText}>+</Text>
                    </Pressable>
                  </View>
                  {alsoSoldBy(catalog, item.listing).map((other) => (
                    <Text key={other.hash} style={styles.hint}>Also sold by {other.seller} · {priceLabel(other.priceSats, rates, currency)}</Text>
                  ))}
                  <TextButton label="Save for later" name={`Save ${item.listing.title} for later`} onPress={() => onSave(item.listing.hash)} />
                </View>
              </View>
            ))}
            <Text style={styles.meta}>{shippingLine(pkg, rates, currency)}</Text>
            <Text style={styles.price}>Package {priceLabel(pkg.totalSats, rates, currency)}</Text>
          </View>
        ))}
      </View>
      <View style={styles.buyBox}>
        <Text style={styles.section}>Summary</Text>
        <Text style={styles.meta}>{formatNumber(itemCount)} items · {formatNumber(packages.length)} {packages.length === 1 ? "seller" : "sellers"}</Text>
        <Text style={styles.meta}>Items {priceLabel(itemsSats, rates, currency)}</Text>
        <Text style={styles.meta}>Shipping {priceLabel(shippingSats, rates, currency)}</Text>
        <Text style={styles.heroPrice}>{priceLabel(itemsSats + shippingSats, rates, currency)}</Text>
        <Button label="Checkout" onPress={onCheckout} />
      </View>
    </ScrollView>
  );
}

function Checkout({
  catalog,
  lines,
  rates,
  currency,
  onOffline,
  onDone,
}: {
  catalog: ShopCatalog;
  lines: CartLine[];
  rates: RateTable;
  currency: CurrencyCode;
  onOffline: () => void;
  onDone: (receipt: CheckoutReceipt, bought: string[]) => void;
}) {
  const packages = cartPackages(catalog, lines);
  const [settlement, setSettlement] = useState<Record<string, "direct" | "escrow">>({});
  const [addresses, setAddresses] = useState<SavedAddress[]>([]);
  const [addressId, setAddressId] = useState<string | "new" | null>(null);
  const [draft, setDraft] = useState<AddressInput>(blankAddress());
  const [saveAddress, setSaveAddress] = useState(true);
  const [note, setNote] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const physical = packages.some((pkg) => pkg.physical);
  const digital = packages.some((pkg) => pkg.digital);
  const itemsSats = packages.reduce((sum, pkg) => sum + pkg.itemsSats, 0);
  const shippingSats = packages.reduce((sum, pkg) => sum + pkg.shippingSats, 0);

  const offlineRef = useRef(onOffline);
  offlineRef.current = onOffline;
  useEffect(() => {
    let stop = false;
    void fetchAddresses()
      .then((next) => {
        if (stop) return;
        setAddresses(next);
        const preferred = next.find((address) => address.default) ?? next[0];
        setAddressId(preferred ? preferred.id : "new");
      })
      .catch((err) => {
        if (err instanceof NodeOfflineError) offlineRef.current();
      });
    return () => {
      stop = true;
    };
  }, []);

  async function submit() {
    if (submitting || packages.length === 0) return;
    const order: Parameters<typeof placeCheckout>[0] = {
      packages: packages.map((pkg) => ({
        settlement: pkg.moderated ? settlement[pkg.seller] ?? "direct" : "direct",
        lines: pkg.items.map((item) => ({ listingHash: item.listing.hash, quantity: item.quantity })),
      })),
    };
    if (physical) {
      if (addressId && addressId !== "new") {
        const chosen = addresses.find((address) => address.id === addressId);
        if (!chosen) {
          setError("Choose a saved address or enter a new one.");
          return;
        }
        order.addressId = chosen.id;
        order.addressLabel = chosen.label;
      } else {
        if (!addressReady(draft)) {
          setError("Enter the label, recipient, street, city, region, postal code, and country.");
          return;
        }
        if (saveAddress) {
          try {
            const savedAddress = await createAddress({ ...draft, default: addresses.length === 0 });
            order.addressId = savedAddress.id;
            order.addressLabel = savedAddress.label;
          } catch (err) {
            if (err instanceof NodeOfflineError) {
              onOffline();
              return;
            }
            setError(err instanceof Error ? err.message : "The address could not be saved.");
            return;
          }
        } else {
          order.addressLabel = draft.label.trim();
          order.shipTo = {
            name: draft.name.trim(),
            street: draft.street.trim(),
            street2: draft.street2?.trim(),
            city: draft.city.trim(),
            region: draft.region.trim(),
            postalCode: draft.postalCode.trim(),
            country: draft.country.trim(),
          };
        }
      }
    }
    if (digital) {
      if (!note.trim()) {
        setError("Enter a delivery note or reply contact.");
        return;
      }
      order.deliveryNote = note.trim();
    }
    setError(null);
    setSubmitting(true);
    try {
      const receipt = await placeCheckout(order);
      onDone(receipt, packages.flatMap((pkg) => pkg.items.map((item) => item.listing.hash)));
    } catch (err) {
      if (err instanceof NodeOfflineError) {
        onOffline();
        return;
      }
      setError(err instanceof Error ? err.message : "The order could not be sealed.");
      setSubmitting(false);
    }
  }

  if (packages.length === 0) {
    return <Text style={styles.status}>Your cart is empty.</Text>;
  }
  return (
    <ScrollView contentContainerStyle={styles.detail} keyboardShouldPersistTaps="handled">
      <View style={styles.detailMain}>
        <Text style={styles.pageTitle}>Checkout</Text>
        <Text style={styles.body}>One order is sealed to each seller. They do not see each other's items.</Text>
        {packages.map((pkg) => (
          <View key={pkg.seller} style={styles.group}>
            <Text style={styles.section}>{pkg.seller}</Text>
            <Text style={styles.meta}>Ships from {pkg.shipFrom}</Text>
            {pkg.items.map((item) => (
              <Text key={item.listing.hash} style={styles.cardTitle}>{item.quantity} × {item.listing.title}</Text>
            ))}
            <Text style={styles.meta}>{shippingLine(pkg, rates, currency)}</Text>
            <Text style={styles.price}>{priceLabel(pkg.totalSats, rates, currency)}</Text>
            {pkg.moderated && catalog.moderator ? (
              <View>
                <Pressable accessibilityRole="button" onPress={() => setSettlement({ ...settlement, [pkg.seller]: "direct" })} style={[styles.choice, (settlement[pkg.seller] ?? "direct") === "direct" && styles.choiceOn]}>
                  <Text style={styles.choiceTitle}>Pay directly</Text>
                </Pressable>
                <Pressable accessibilityRole="button" onPress={() => setSettlement({ ...settlement, [pkg.seller]: "escrow" })} style={[styles.choice, settlement[pkg.seller] === "escrow" && styles.choiceOn]}>
                  <Text style={styles.choiceTitle}>Moderated escrow</Text>
                  <Text style={styles.meta}>You and the vendor both sign. {catalog.moderator.name} is told only if a dispute is opened.</Text>
                </Pressable>
              </View>
            ) : (
              <Text style={styles.meta}>Pay directly</Text>
            )}
          </View>
        ))}
        {physical ? (
          <View>
            <Text style={styles.section}>Ship to</Text>
            <Text style={styles.meta}>This address goes to every physical package.</Text>
            {addresses.map((address) => (
              <Pressable key={address.id} accessibilityRole="button" onPress={() => setAddressId(address.id)} style={[styles.choice, addressId === address.id && styles.choiceOn]}>
                <Text style={styles.choiceTitle}>{address.label}{address.default ? " · Default" : ""}</Text>
                <Text style={styles.meta}>{address.name}, {address.city}</Text>
              </Pressable>
            ))}
            <Pressable accessibilityRole="button" onPress={() => setAddressId("new")} style={[styles.choice, addressId === "new" && styles.choiceOn]}>
              <Text style={styles.choiceTitle}>New address</Text>
            </Pressable>
            {addressId === "new" ? (
              <View>
                <AddressFields value={draft} onChange={setDraft} />
                <Pressable accessibilityRole="checkbox" accessibilityState={{ checked: saveAddress }} onPress={() => setSaveAddress((value) => !value)} style={styles.checkRow}>
                  <View style={[styles.check, saveAddress && styles.checkOn]} />
                  <Text style={styles.body}>Save this address on this node</Text>
                </Pressable>
              </View>
            ) : null}
          </View>
        ) : null}
        {digital ? <Field label="Delivery note or reply contact" value={note} onChangeText={setNote} multiline /> : null}
        {error ? <Text style={styles.error}>{error}</Text> : null}
        <Button label={submitting ? "Sealing" : "Place order"} onPress={() => void submit()} />
      </View>
      <View style={styles.buyBox}>
        <Text style={styles.section}>Summary</Text>
        <Text style={styles.meta}>Items {priceLabel(itemsSats, rates, currency)}</Text>
        <Text style={styles.meta}>Shipping {priceLabel(shippingSats, rates, currency)}</Text>
        <Text style={styles.heroPrice}>{priceLabel(itemsSats + shippingSats, rates, currency)}</Text>
      </View>
    </ScrollView>
  );
}

function Confirmation({
  receipt,
  rates,
  currency,
  onOrders,
  onShop,
}: {
  receipt: CheckoutReceipt;
  rates: RateTable;
  currency: CurrencyCode;
  onOrders: () => void;
  onShop: () => void;
}) {
  return (
    <ScrollView contentContainerStyle={styles.sheetPad}>
      <Text style={styles.pageTitle}>Sealed and accepted</Text>
      <Text style={styles.body}>Each seller is paid separately. One payment would tie those sellers to the same buyer on the chain.</Text>
      <Text style={styles.heroPrice}>{priceLabel(receipt.totalSats, rates, currency)}</Text>
      <Text style={styles.meta}>Items {priceLabel(receipt.itemsSats, rates, currency)} · Shipping {priceLabel(receipt.shippingSats, rates, currency)}</Text>
      {receipt.orders.map((order) => (
        <View key={order.orderId} style={styles.group}>
          <Text style={styles.section}>{order.seller}</Text>
          <Text style={styles.body}>Awaiting payment</Text>
          {order.addressLabel ? <Text style={styles.meta}>Ship to {order.addressLabel}</Text> : null}
          {order.lines.map((line) => (
            <Text key={line.listingHash} style={styles.cardTitle}>{line.quantity} × {line.title} · {priceLabel(line.priceSats, rates, currency)}</Text>
          ))}
          <Text style={styles.meta}>Shipping {priceLabel(order.shippingSats, rates, currency)}</Text>
          <Text style={styles.price}>{priceLabel(order.totalSats, rates, currency)}</Text>
          <Text style={styles.meta}>{order.settlement === "escrow" ? "Moderated escrow" : "Direct pay"}</Text>
          <PaymentBlock settlement={order.settlement} payment={order.payment} escrow={order.escrow} />
        </View>
      ))}
      <Text style={styles.note}>No transaction has been broadcast.</Text>
      <Button label="View orders" onPress={onOrders} />
      <TextButton label="Back to the shop" onPress={onShop} />
    </ScrollView>
  );
}

function Addresses({ onOffline }: { onOffline: () => void }) {
  const [addresses, setAddresses] = useState<SavedAddress[]>([]);
  const [draft, setDraft] = useState<AddressInput>(blankAddress());
  const [editing, setEditing] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const offlineRef = useRef(onOffline);
  offlineRef.current = onOffline;
  const refresh = useCallback(async () => {
    try {
      setAddresses(await fetchAddresses());
    } catch (err) {
      if (err instanceof NodeOfflineError) offlineRef.current();
      else setError(err instanceof Error ? err.message : "The address book could not be read.");
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function save() {
    if (!addressReady(draft)) {
      setError("Enter the label, recipient, street, city, region, postal code, and country.");
      return;
    }
    setError(null);
    try {
      if (editing) await updateAddress(editing, draft);
      else await createAddress({ ...draft, default: addresses.length === 0 || draft.default });
      setDraft(blankAddress());
      setEditing(null);
      await refresh();
    } catch (err) {
      if (err instanceof NodeOfflineError) onOffline();
      else setError(err instanceof Error ? err.message : "The address could not be saved.");
    }
  }

  return (
    <ScrollView contentContainerStyle={styles.sheetPad}>
      <Text style={styles.pageTitle}>Addresses</Text>
      <Text style={styles.body}>{ADDRESS_NOTE}</Text>
      {addresses.map((address) => (
        <View key={address.id} style={styles.group}>
          <Text style={styles.cardTitle}>{address.label}{address.default ? " · Default" : ""}</Text>
          <Text style={styles.meta}>{address.name}</Text>
          <Text style={styles.meta}>{address.city}, {address.region} {address.postalCode}</Text>
          <Text style={styles.meta}>{address.country}</Text>
          <View style={styles.row}>
            <TextButton label="Edit" onPress={() => { setEditing(address.id); setDraft(address); }} />
            <TextButton
              label="Set default"
              onPress={() => {
                void updateAddress(address.id, { ...address, default: true }).then(refresh).catch((err) => {
                  if (err instanceof NodeOfflineError) onOffline();
                });
              }}
            />
            <TextButton
              label="Delete"
              onPress={() => {
                void deleteAddress(address.id).then(refresh).catch((err) => {
                  if (err instanceof NodeOfflineError) onOffline();
                });
              }}
            />
          </View>
        </View>
      ))}
      <Text style={styles.section}>{editing ? "Edit address" : "Add address"}</Text>
      <AddressFields value={draft} onChange={setDraft} />
      {error ? <Text style={styles.error}>{error}</Text> : null}
      <Button label={editing ? "Save address" : "Add address"} onPress={() => void save()} />
    </ScrollView>
  );
}

function SafetyScreen({
  settings,
  onChange,
  labelers,
  onLabelers,
}: {
  settings: SafetySettings;
  onChange: (settings: SafetySettings) => void;
  labelers: LabelerState | null;
  onLabelers: (state: LabelerState) => void;
}) {
  const [text, setText] = useState(() => settings.blockedKeywords.join("\n"));
  const [labelerKey, setLabelerKey] = useState("");
  const [labelerError, setLabelerError] = useState("");
  const followed = labelers?.labelers ?? [];
  const saveLabelers = (next: { key: string; url: string | null }[]) => {
    setLabelerError("");
    setLabelers(next).then(
      (state) => {
        onLabelers(state);
        setLabelerKey("");
      },
      (error: unknown) => setLabelerError(error instanceof Error ? error.message : "The labelers could not be saved."),
    );
  };
  const current = followed.map((item) => ({ key: item.key, url: item.url }));
  return (
    <ScrollView contentContainerStyle={styles.sheetPad}>
      <Text style={styles.pageTitle}>Content filters</Text>
      <Text style={styles.body}>These filters run on this device. They are not sent to your node or anyone else, and they do not remove anything from the network.</Text>
      <Pressable accessibilityRole="checkbox" accessibilityState={{ checked: settings.safeMode }} onPress={() => onChange({ ...settings, safeMode: !settings.safeMode })} style={styles.checkRow}>
        <View style={[styles.check, settings.safeMode && styles.checkOn]} />
        <Text style={styles.body}>Safe mode</Text>
      </Pressable>
      <Text style={styles.meta}>Safe mode applies the rating and tag choices below, and treats unlabeled listings that use a short list of explicit words as sensitive.</Text>
      <Options label="Sensitive listings" value={settings.sensitive} options={["hide", "cover", "show"]} onChange={(sensitive) => onChange({ ...settings, sensitive })} />
      <Options label="Adult listings" value={settings.adult} options={["hide", "show"]} onChange={(adult) => onChange({ ...settings, adult })} />
      {CONTENT_TAGS.map((tag) => (
        <Options key={tag} label={`Tagged ${tag}`} value={settings.tags[tag]} options={["show", "hide"]} onChange={(mode) => onChange({ ...settings, tags: { ...settings.tags, [tag]: mode } })} />
      ))}
      <Field
        label="Blocked words, one per line"
        value={text}
        multiline
        onChangeText={(next) => {
          setText(next);
          onChange({ ...settings, blockedKeywords: parseKeywords(next) });
        }}
      />
      <Text style={styles.meta}>Listings whose title, category, description, or tags contain one of these whole words are hidden, even with safe mode off.</Text>
      <Text style={styles.section}>Hidden</Text>
      <Text style={styles.meta}>Sellers and listings you hid stay hidden even with safe mode off. A seller is remembered by its store key.</Text>
      {settings.hiddenStores.length + settings.hiddenListings.length === 0 ? <Text style={styles.meta}>Nothing hidden.</Text> : null}
      {settings.hiddenStores.map((item) => (
        <TextButton key={item.id} label={`Unhide seller ${item.label}`} onPress={() => onChange({ ...settings, hiddenStores: toggleHidden(settings.hiddenStores, item) })} />
      ))}
      {settings.hiddenListings.map((item) => (
        <TextButton key={item.id} label={`Unhide listing ${item.label}`} onPress={() => onChange({ ...settings, hiddenListings: toggleHidden(settings.hiddenListings, item) })} />
      ))}
      <Text style={styles.section}>Labelers</Text>
      <Text style={styles.meta}>Follow a labeler and safe mode uses its signed labels too, whichever is strictest. Labelers only hide or cover things on this device. You follow none by default.</Text>
      {followed.map((item) => (
        <View key={item.key}>
          <Text style={styles.meta}>{`${item.name ?? "Unknown labeler"} ${item.key.slice(0, 16)}…${item.error ? ` — ${item.error}` : ""}`}</Text>
          <TextButton label="Unfollow" onPress={() => saveLabelers(current.filter((entry) => entry.key !== item.key))} />
        </View>
      ))}
      {labelers?.samples
        .filter((sample) => !followed.some((item) => item.key === sample.key))
        .map((sample) => (
          <TextButton key={sample.key} label={`Follow sample: ${sample.name}`} onPress={() => saveLabelers([...current, { key: sample.key, url: null }])} />
        ))}
      <Field label="Labeler key" value={labelerKey} onChangeText={(next) => setLabelerKey(next.trim())} />
      <TextButton label="Follow this key" onPress={() => saveLabelers([...current, { key: labelerKey, url: null }])} />
      {labelerError ? <Text style={styles.error}>{labelerError}</Text> : null}
    </ScrollView>
  );
}

function Options<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: readonly T[]; onChange: (value: T) => void }) {
  return (
    <View style={styles.row}>
      <Text style={styles.label}>{label}</Text>
      {options.map((option) => (
        <Pressable key={option} accessibilityRole="button" accessibilityState={{ selected: value === option }} onPress={() => onChange(option)} style={[styles.sort, value === option && styles.sortOn]}>
          <Text style={[styles.sortText, value === option && styles.sortTextOn]}>{option}</Text>
        </Pressable>
      ))}
    </View>
  );
}

function orderGroups(orders: BuyerOrder[]): BuyerOrder[][] {
  const groups = new Map<string, BuyerOrder[]>();
  for (const order of orders) {
    const key = order.checkoutId || order.id;
    const group = groups.get(key) ?? [];
    group.push(order);
    groups.set(key, group);
  }
  return [...groups.values()].sort((left, right) => Math.max(...right.map((order) => order.created)) - Math.max(...left.map((order) => order.created)));
}

function Orders({
  rates,
  currency,
  onOpen,
  onOffline,
}: {
  rates: RateTable;
  currency: CurrencyCode;
  onOpen: (id: string) => void;
  onOffline: () => void;
}) {
  const [orders, setOrders] = useState<BuyerOrder[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const offlineRef = useRef(onOffline);
  offlineRef.current = onOffline;
  useEffect(() => {
    void fetchOrders()
      .then(setOrders)
      .catch((err) => {
        if (err instanceof NodeOfflineError) offlineRef.current();
        else setError(err instanceof Error ? err.message : "The orders could not be read.");
      });
  }, []);
  const groups = orders ? orderGroups(orders) : [];
  return (
    <ScrollView contentContainerStyle={styles.sheetPad}>
      <Text style={styles.pageTitle}>Orders</Text>
      {error ? <Text style={styles.error}>{error}</Text> : null}
      {orders && orders.length === 0 ? <Text style={styles.body}>No orders yet.</Text> : null}
      {groups.map((group) => {
        const id = group[0].checkoutId || group[0].id;
        const when = Math.max(...group.map((order) => order.created));
        const total = group.reduce((sum, order) => sum + order.totalSats, 0);
        const sellers = group.length === 1 ? "1 seller" : `${formatNumber(group.length)} sellers`;
        return (
          <View key={id} style={styles.group}>
            <Pressable accessibilityRole="button" accessibilityLabel={`${sellers}, ${priceLabel(total, rates, currency)}`} onPress={() => setOpen(open === id ? null : id)}>
              <Text style={styles.meta}>{new Date(when * 1000).toLocaleString()}</Text>
              <Text style={styles.cardTitle}>{sellers}</Text>
              <Text style={styles.price}>{priceLabel(total, rates, currency)}</Text>
            </Pressable>
            {open === id ? group.map((order) => (
              <Pressable key={order.id} accessibilityRole="button" onPress={() => onOpen(order.id)} style={styles.choice}>
                <Text style={styles.choiceTitle}>{order.seller}</Text>
                <Text style={styles.meta}>{order.status} · {order.settlement === "escrow" ? "Moderated escrow" : "Direct pay"}</Text>
                <Text style={styles.price}>{priceLabel(order.totalSats, rates, currency)}</Text>
              </Pressable>
            )) : null}
          </View>
        );
      })}
    </ScrollView>
  );
}

function OrderDetail({
  id,
  rates,
  currency,
  onOffline,
}: {
  id: string;
  rates: RateTable;
  currency: CurrencyCode;
  onOffline: () => void;
}) {
  const [order, setOrder] = useState<BuyerOrder | null>(null);
  const [error, setError] = useState<string | null>(null);
  const offlineRef = useRef(onOffline);
  offlineRef.current = onOffline;
  useEffect(() => {
    void fetchOrder(id)
      .then(setOrder)
      .catch((err) => {
        if (err instanceof NodeOfflineError) offlineRef.current();
        else setError(err instanceof Error ? err.message : "That order could not be read.");
      });
  }, [id]);
  if (!order) {
    return <Text style={styles.status}>{error ?? "Loading the order"}</Text>;
  }
  return (
    <ScrollView contentContainerStyle={styles.sheetPad}>
      <Text style={styles.kicker}>{new Date(order.created * 1000).toLocaleString()}</Text>
      <Text style={styles.pageTitle}>{order.seller}</Text>
      <Text style={styles.body}>{order.status}</Text>
      {order.addressLabel ? <Text style={styles.meta}>Ship to {order.addressLabel}</Text> : null}
      {order.lines.map((line) => (
        <Text key={line.listingHash} style={styles.cardTitle}>{line.quantity} × {line.title} · {priceLabel(line.priceSats, rates, currency)}</Text>
      ))}
      {order.itemsSats > 0 ? <Text style={styles.meta}>Items {priceLabel(order.itemsSats, rates, currency)}</Text> : null}
      <Text style={styles.meta}>Shipping {priceLabel(order.shippingSats, rates, currency)}</Text>
      <Text style={styles.heroPrice}>{priceLabel(order.totalSats, rates, currency)}</Text>
      <Text style={styles.meta}>{order.settlement === "escrow" ? "Moderated escrow" : "Direct pay"}</Text>
      <PaymentBlock settlement={order.settlement} payment={order.payment} escrow={order.escrow} />
      <Text style={styles.note}>No transaction has been broadcast.</Text>
    </ScrollView>
  );
}

function PaymentBlock({
  settlement,
  payment,
  escrow,
}: {
  settlement: "direct" | "escrow";
  payment: BuyerOrder["payment"];
  escrow: BuyerOrder["escrow"];
}) {
  if (settlement === "direct" && payment) {
    return (
      <View>
        <CopyLine label="Lightning offer" value={payment.lightning} />
        <CopyLine label="Silent payment" value={payment.onchain} />
      </View>
    );
  }
  if (escrow) {
    return (
      <View>
        <Text style={styles.body}>Fund {formatBtc(escrow.fundingSats)}.</Text>
        <Text style={styles.body}>Timeout at block {formatNumber(escrow.timeoutHeight)}.</Text>
        <Text style={styles.body}>Two signatures are required.</Text>
        <CopyLine label="Funding address" value={escrow.address} />
      </View>
    );
  }
  return null;
}

function AddressFields({ value, onChange }: { value: AddressInput; onChange: (value: AddressInput) => void }) {
  const set = (key: keyof AddressInput) => (next: string) => onChange({ ...value, [key]: next });
  return (
    <View>
      <Field label="Label" value={value.label} onChangeText={set("label")} />
      <Field label="Recipient" value={value.name} onChangeText={set("name")} />
      <Field label="Street" value={value.street} onChangeText={set("street")} />
      <Field label="Street line 2" value={value.street2 ?? ""} onChangeText={set("street2")} />
      <Field label="City" value={value.city} onChangeText={set("city")} />
      <Field label="Region" value={value.region} onChangeText={set("region")} />
      <Field label="Postal code" value={value.postalCode} onChangeText={set("postalCode")} />
      <Field label="Country" value={value.country} onChangeText={set("country")} />
    </View>
  );
}

function SearchField({ query, onQuery }: { query: string; onQuery: (value: string) => void }) {
  return (
    <TextInput
      value={query}
      onChangeText={onQuery}
      placeholder="Search"
      placeholderTextColor={MUTED}
      accessibilityLabel="Search listings"
      autoCapitalize="none"
      autoCorrect={false}
      style={styles.search}
    />
  );
}

function Field({
  label,
  value,
  onChangeText,
  keyboard,
  multiline,
}: {
  label: string;
  value: string;
  onChangeText: (value: string) => void;
  keyboard?: "number-pad";
  multiline?: boolean;
}) {
  return (
    <View style={styles.field}>
      <Text style={styles.label}>{label}</Text>
      <TextInput value={value} onChangeText={onChangeText} accessibilityLabel={label} keyboardType={keyboard} multiline={multiline} autoCapitalize="none" style={[styles.input, multiline && styles.inputMulti]} />
    </View>
  );
}

function CopyLine({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <View style={styles.copy}>
      <Text style={styles.label}>{label}</Text>
      <Text style={styles.copyValue} selectable>{value}</Text>
      <TextButton
        label={copied ? "Copied" : "Copy"}
        onPress={() => {
          const clipboard = (globalThis as { navigator?: { clipboard?: { writeText: (text: string) => Promise<void> } } }).navigator?.clipboard;
          void clipboard?.writeText(value).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          });
        }}
      />
    </View>
  );
}

function Button({ label, onPress }: { label: string; onPress: () => void }) {
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={label} onPress={onPress} style={(state) => [styles.button, pressed(state) && styles.pressed]}>
      <Text style={styles.buttonText}>{label}</Text>
    </Pressable>
  );
}

function TextButton({ label, name, onPress }: { label: string; name?: string; onPress: () => void }) {
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={name ?? label} onPress={onPress} style={styles.textButton}>
      <Text style={styles.textButtonLabel}>{label}</Text>
    </Pressable>
  );
}

function IconCount({ label, icon, count, onPress }: { label: string; icon: string; count: number; onPress: () => void }) {
  return (
    <Pressable accessibilityRole="button" accessibilityLabel={`${label}, ${count}`} onPress={onPress} style={styles.iconButton}>
      <Image source={{ uri: icon }} style={styles.icon} />
      <Text style={styles.iconCount}>{formatNumber(count)}</Text>
    </Pressable>
  );
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

function pressed(state: PressableStateCallbackType): boolean {
  return state.pressed || ("hovered" in state && state.hovered === true);
}

const styles = StyleSheet.create({
  page: { flex: 1, backgroundColor: PAGE, ...(Platform.OS === "web" ? ({ height: "100vh", overflow: "hidden" } as unknown as ViewStyle) : null) },
  fill: { flex: 1 },
  trust: { minHeight: 36, backgroundColor: NAVY, flexDirection: "row", flexWrap: "wrap", justifyContent: "center", alignItems: "center", gap: 16, paddingHorizontal: 16, paddingVertical: 8 },
  trustText: { color: "#f4f8fb", fontFamily: SANS, fontSize: 13, lineHeight: 16 },
  header: { minHeight: 64, backgroundColor: CARD, borderBottomWidth: 1, borderBottomColor: LINE, flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 12, paddingHorizontal: 20, paddingVertical: 12 },
  brand: { flexDirection: "row", alignItems: "center", gap: 8 },
  mark: { width: 32, height: 32 },
  wordmark: { fontFamily: SANS, fontSize: 20, lineHeight: 24, fontWeight: "500", color: INK },
  search: { flexGrow: 1, flexBasis: 220, minHeight: 44, backgroundColor: SOFT, borderRadius: 8, paddingHorizontal: 12, fontFamily: SANS, fontSize: 16, color: INK },
  headerTools: { flexDirection: "row", alignItems: "center", gap: 8, marginLeft: "auto" },
  currencies: { flexDirection: "row", gap: 4 },
  currency: { minHeight: 32, paddingHorizontal: 8, justifyContent: "center", borderRadius: 6 },
  currencyOn: { backgroundColor: SOFT },
  currencyText: { fontFamily: SANS, fontSize: 12, color: MUTED },
  currencyTextOn: { color: INK, fontWeight: "500" },
  iconButton: { minHeight: 36, flexDirection: "row", alignItems: "center", gap: 4, paddingHorizontal: 4 },
  icon: { width: 20, height: 20 },
  iconCount: { fontFamily: SANS, fontSize: 13, color: INK },
  you: { minHeight: 36, paddingHorizontal: 8, justifyContent: "center" },
  youText: { fontFamily: SANS, fontSize: 14, color: INK },
  menu: { position: "absolute", top: 36, right: 0, backgroundColor: CARD, borderWidth: 1, borderColor: LINE, borderRadius: 8, padding: 8, zIndex: 4, minWidth: 140 },
  browse: { flex: 1, flexDirection: "row" },
  rail: { width: 240, borderRightWidth: 1, borderRightColor: LINE, backgroundColor: CARD },
  railBody: { padding: 16, gap: 16 },
  filterGroup: { gap: 4 },
  filterLabel: { fontFamily: SANS, fontSize: 13, color: MUTED, marginBottom: 4 },
  filterRow: { minHeight: 32, justifyContent: "center", paddingHorizontal: 8, borderRadius: 6 },
  filterRowOn: { backgroundColor: SOFT },
  filterText: { fontFamily: SANS, fontSize: 14, color: SECONDARY },
  filterTextOn: { color: BLUE, fontWeight: "500" },
  priceRow: { flexDirection: "row", gap: 8 },
  priceInput: { flex: 1, minHeight: 36, borderWidth: 1, borderColor: LINE, borderRadius: 8, paddingHorizontal: 8, fontFamily: SANS, color: INK, backgroundColor: CARD },
  filterSheet: { position: "absolute", top: 0, left: 0, right: 0, bottom: 0, backgroundColor: CARD, zIndex: 5 },
  filterSheetHead: { flexDirection: "row", justifyContent: "space-between", alignItems: "center", paddingHorizontal: 16, paddingTop: 16 },
  results: { padding: 24, gap: 16, width: "100%", maxWidth: "100%" },
  hero: { flexDirection: "row", gap: 20, backgroundColor: CARD, borderRadius: 12, overflow: "hidden", borderWidth: 1, borderColor: LINE },
  heroCompact: { flexDirection: "column", gap: 0 },
  heroImage: { width: 220, height: 220, backgroundColor: SOFT },
  heroImageCompact: { width: "100%", height: 200 },
  heroCopy: { flex: 1, padding: 20, justifyContent: "center", gap: 8 },
  heroTitle: { fontFamily: SANS, fontSize: 32, lineHeight: 36, color: INK },
  heroTitleCompact: { fontSize: 24, lineHeight: 30 },
  heroSeller: { fontFamily: SANS, fontSize: 14, color: SECONDARY },
  heroPrice: { fontFamily: SANS, fontSize: 20, lineHeight: 28, fontWeight: "500", color: INK },
  crumb: { fontFamily: SANS, fontSize: 13, color: MUTED },
  resultBar: { flexDirection: "row", flexWrap: "wrap", justifyContent: "space-between", alignItems: "center", gap: 8 },
  resultCount: { fontFamily: SANS, fontSize: 16, color: INK },
  resultTools: { flexGrow: 1, flexBasis: 220, flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 4 },
  sort: { minHeight: 32, paddingHorizontal: 8, justifyContent: "center", borderRadius: 6 },
  sortOn: { backgroundColor: SOFT },
  sortText: { fontFamily: SANS, fontSize: 13, color: SECONDARY },
  sortTextOn: { color: INK, fontWeight: "500" },
  grid: { flexDirection: "row", flexWrap: "wrap", gap: 16 },
  list: { gap: 12 },
  card: { backgroundColor: CARD, borderRadius: 12, overflow: "hidden" },
  cardRow: { flexDirection: "row" },
  cardImageWrap: { width: "100%", aspectRatio: 1, backgroundColor: SOFT },
  cardImageWide: { width: 120, height: 120, backgroundColor: SOFT },
  cardImage: { width: "100%", height: "100%" },
  cardBody: { padding: 12, gap: 4, flex: 1 },
  badgeRow: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 6 },
  badge: { fontFamily: SANS, fontSize: 12, lineHeight: 16, color: BLUE },
  heart: { marginLeft: "auto", width: 28, height: 28, alignItems: "center", justifyContent: "center" },
  heartIcon: { width: 18, height: 18 },
  seller: { fontFamily: SANS, fontSize: 13, color: MUTED },
  cardTitle: { fontFamily: SANS, fontSize: 16, lineHeight: 22, fontWeight: "500", color: INK },
  price: { fontFamily: SANS, fontSize: 14, lineHeight: 20, color: INK },
  kicker: { fontFamily: SANS, fontSize: 13, color: MUTED },
  pageTitle: { fontFamily: SANS, fontSize: 32, lineHeight: 36, color: INK },
  body: { fontFamily: SANS, fontSize: 16, lineHeight: 24, color: SECONDARY },
  meta: { fontFamily: SANS, fontSize: 13, lineHeight: 18, color: MUTED },
  detail: { flexDirection: "row", flexWrap: "wrap", gap: 24, padding: 24 },
  detailMain: { flexGrow: 1, flexBasis: 320, gap: 8, maxWidth: 720 },
  detailImage: { width: "100%", aspectRatio: 1, borderRadius: 12, backgroundColor: SOFT, maxWidth: 480 },
  thumbs: { flexDirection: "row", gap: 8 },
  thumb: { width: 64, height: 64, borderRadius: 8, backgroundColor: SOFT },
  thumbOn: { borderWidth: 2, borderColor: BLUE },
  buyBox: { width: 300, maxWidth: "100%", backgroundColor: CARD, borderRadius: 12, padding: 16, borderWidth: 1, borderColor: LINE, gap: 8 },
  sheetPad: { padding: 24, gap: 12, maxWidth: 760 },
  group: { backgroundColor: CARD, borderRadius: 12, padding: 16, gap: 8, borderWidth: 1, borderColor: LINE },
  cartLine: { flexDirection: "row", gap: 12 },
  cartImage: { width: 72, height: 72, borderRadius: 8, backgroundColor: SOFT },
  packageHead: { flexDirection: "row", justifyContent: "space-between", alignItems: "flex-start", gap: 12 },
  stepper: { flexDirection: "row", alignItems: "center", gap: 8 },
  step: { width: 32, height: 32, borderRadius: 8, borderWidth: 1, borderColor: LINE, alignItems: "center", justifyContent: "center" },
  stepText: { fontFamily: SANS, fontSize: 18, lineHeight: 22, color: INK },
  stepValue: { fontFamily: SANS, fontSize: 14, color: INK, minWidth: 16, textAlign: "center" },
  hint: { fontFamily: SANS, fontSize: 13, lineHeight: 18, color: MUTED },
  section: { fontFamily: SANS, fontSize: 18, lineHeight: 24, fontWeight: "500", color: INK },
  field: { gap: 4, marginTop: 8 },
  label: { fontFamily: SANS, fontSize: 13, color: MUTED },
  input: { minHeight: 44, borderWidth: 1, borderColor: LINE, borderRadius: 8, paddingHorizontal: 12, fontFamily: SANS, fontSize: 16, color: INK, backgroundColor: CARD },
  inputMulti: { minHeight: 88, paddingVertical: 8 },
  qty: { width: 72, minHeight: 36, borderWidth: 1, borderColor: LINE, borderRadius: 8, paddingHorizontal: 8, fontFamily: SANS, color: INK },
  choice: { borderWidth: 1, borderColor: LINE, borderRadius: 8, padding: 12, marginTop: 8, backgroundColor: CARD },
  choiceOn: { borderColor: BLUE, backgroundColor: "#e7f1f8" },
  choiceTitle: { fontFamily: SANS, fontSize: 15, color: INK },
  checkRow: { flexDirection: "row", alignItems: "center", gap: 8, marginTop: 8 },
  check: { width: 16, height: 16, borderRadius: 4, borderWidth: 1, borderColor: MUTED },
  checkOn: { backgroundColor: BLUE, borderColor: BLUE },
  button: { marginTop: 12, minHeight: 44, borderRadius: 8, backgroundColor: BLUE, alignItems: "center", justifyContent: "center", paddingHorizontal: 16 },
  pressed: { opacity: 0.88 },
  buttonText: { fontFamily: SANS, fontSize: 16, fontWeight: "500", color: "#f4f8fb" },
  textButton: { minHeight: 32, justifyContent: "center", paddingHorizontal: 4 },
  textButtonLabel: { fontFamily: SANS, fontSize: 14, color: BLUE },
  error: { fontFamily: SANS, fontSize: 14, lineHeight: 20, color: ERROR, marginTop: 8 },
  note: { fontFamily: SANS, fontSize: 14, color: SECONDARY, marginTop: 8 },
  status: { margin: 32, fontFamily: SANS, fontSize: 16, color: SECONDARY },
  emptyTitle: { fontFamily: SANS, fontSize: 24, lineHeight: 28, color: INK },
  row: { flexDirection: "row", gap: 12, flexWrap: "wrap" },
  copy: { marginTop: 12, gap: 4 },
  copyValue: { fontFamily: SANS, fontSize: 13, lineHeight: 18, color: INK },
  offline: { flex: 1, justifyContent: "center", padding: 32, gap: 8, backgroundColor: NAVY },
  offlineMark: { width: 56, height: 56 },
  offlineTitle: { fontFamily: SANS, fontSize: 32, color: "#f4f8fb" },
  offlineBody: { fontFamily: SANS, fontSize: 16, lineHeight: 24, color: "#f4f8fb", maxWidth: 460 },
});
