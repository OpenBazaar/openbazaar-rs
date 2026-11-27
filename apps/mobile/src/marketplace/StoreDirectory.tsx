import { useMemo, useState } from "react";
import { fetchDirectory } from "../follows";
import { fetchStores } from "../stores";
import { DEFAULT_OPTIONS, directoryCategories, filterAndSortStores, filtersActive, type DirectoryRail, type DirectorySort } from "../storeDirectory";
import { useNodeResource } from "../useNodeResource";
import { FollowStoreDialog } from "./FollowStoreDialog";
import { Empty, Icon } from "./Marketplace";
import { StoreDirectoryCard } from "./StoreDirectoryCard";

const RAILS: [DirectoryRail, string][] = [["all", "All rails"], ["btc", "Bitcoin"], ["ln", "Lightning"], ["xmr", "Monero"], ["zec", "Zcash"]];
const SORTS: [DirectorySort, string][] = [["listings", "Most listings"], ["rating", "Top rated"], ["name", "Name A–Z"]];
const plural = (n: number) => `${n.toLocaleString("en-US")} store${n === 1 ? "" : "s"}`;

/** Every store this node knows, to search, filter, follow, and open. */
export function StoreDirectory({ onOpen, onFollowChanged }: { onOpen: (storeId: string) => void; onFollowChanged: (message: string) => void }) {
  const stores = useNodeResource(fetchStores, 15000);
  const directory = useNodeResource(fetchDirectory, 15000);
  const [filters, setFilters] = useState(DEFAULT_OPTIONS);
  const [adding, setAdding] = useState(false);
  const all = useMemo(() => stores.data?.stores ?? [], [stores.data]);
  const followedIds = useMemo(() => new Set((directory.data ?? []).filter(item => item.added).map(item => item.storeId)), [directory.data]);
  const visible = useMemo(() => filterAndSortStores(all, { ...filters, followedIds }), [all, filters, followedIds]);
  const everyone = useMemo(() => filterAndSortStores(all, { ...filters, scope: "all", followedIds }).length, [all, filters, followedIds]);
  const followed = useMemo(() => filterAndSortStores(all, { ...filters, scope: "following", followedIds }).length, [all, filters, followedIds]);
  const categories = useMemo(() => directoryCategories(all), [all]);
  const set = (change: Partial<typeof DEFAULT_OPTIONS>) => setFilters(current => ({ ...current, ...change }));
  const shared = { stores: directory.data, onUpdate: directory.update };
  const refreshed = (message: string) => {
    void directory.refresh();
    void stores.refresh();
    onFollowChanged(message);
  };

  return <section className="ob-directory" aria-labelledby="ob-directory-title">
    <header className="ob-directory-header">
      <div>
        <h1 id="ob-directory-title">Stores<span aria-label={plural(visible.length)}>{visible.length}</span></h1>
        <p>Every store your node knows. Open one to browse its goods, or follow it to keep its records.</p>
      </div>
      <button type="button" className="ob-secondary" onClick={() => setAdding(true)}>Follow a store</button>
    </header>
    <div className="ob-directory-controls">
      <label className="ob-search ob-directory-search">
        <Icon name="search" size={18}/>
        <input aria-label="Search stores" placeholder="Search stores by name, region, or category…" value={filters.query} onChange={event => set({ query: event.target.value })}/>
        {filters.query && <button type="button" className="ob-icon-button" aria-label="Clear search" onClick={() => set({ query: "" })}><Icon name="close" size={16}/></button>}
      </label>
      <div className="ob-directory-segment" role="group" aria-label="Which stores">
        <button type="button" aria-pressed={filters.scope === "all"} onClick={() => set({ scope: "all" })}>All stores <span>{everyone}</span></button>
        <button type="button" aria-pressed={filters.scope === "following"} onClick={() => set({ scope: "following" })}>Following <span>{followed}</span></button>
      </div>
      <div className="ob-directory-selects">
        <label className="ob-directory-select"><span className="ob-sr-only">Category</span>
          <select aria-label="Category" value={filters.category} onChange={event => set({ category: event.target.value })}>
            <option value="">All categories</option>
            {categories.map(category => <option key={category.name} value={category.name}>{category.name} ({category.count})</option>)}
          </select>
        </label>
        <label className="ob-directory-select"><span className="ob-sr-only">Payment rail</span>
          <select aria-label="Payment rail" value={filters.rail} onChange={event => set({ rail: event.target.value as DirectoryRail })}>
            {RAILS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        <label className="ob-directory-select"><span className="ob-sr-only">Sort stores</span>
          <select aria-label="Sort stores" value={filters.sort} onChange={event => set({ sort: event.target.value as DirectorySort })}>
            {SORTS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
        {filtersActive(filters) && <button type="button" className="ob-text-link" onClick={() => setFilters(current => ({ ...DEFAULT_OPTIONS, sort: current.sort }))}>Clear filters</button>}
      </div>
    </div>
    {stores.error && <p className="ob-form-error" role="alert">{stores.error}</p>}
    {stores.loading && !stores.data ? <p role="status">Loading stores…</p> : !stores.data ? null : visible.length ? <>
      <p className="ob-directory-status" role="status" aria-live="polite">{visible.length === all.length ? plural(all.length) : `${visible.length.toLocaleString("en-US")} of ${plural(all.length)}`}</p>
      <div className="ob-directory-grid">{visible.map(store => <StoreDirectoryCard key={store.key} store={store} onOpen={onOpen} onFollowChanged={refreshed} directory={shared}/>)}</div>
    </> : <DirectoryEmpty empty={all.length === 0} following={filters.scope === "following" && filtersActive({ ...filters, scope: "all" }) === false} onClear={() => setFilters(current => ({ ...DEFAULT_OPTIONS, sort: current.sort }))} onAll={() => set({ scope: "all" })} onFollow={() => setAdding(true)}/>}
    {adding && <FollowStoreDialog onClose={() => setAdding(false)} onFollowed={refreshed}/>}
  </section>;
}

function DirectoryEmpty({ empty, following, onClear, onAll, onFollow }: { empty: boolean; following: boolean; onClear: () => void; onAll: () => void; onFollow: () => void }) {
  if (empty) return <Empty icon="store" title="No stores yet" text="Stores your node knows show up here. Follow one by its key or address to start."><button type="button" className="ob-primary" onClick={onFollow}>Follow a store</button></Empty>;
  if (following) return <Empty icon="heart" title="You are not following any stores" text="Follow a store to keep its records on your node and find it here."><button type="button" className="ob-secondary" onClick={onAll}>Browse all stores</button><button type="button" className="ob-primary" onClick={onFollow}>Follow a store</button></Empty>;
  return <Empty icon="search" title="No stores match" text="Try a different search, category, or payment rail."><button type="button" className="ob-primary" onClick={onClear}>Clear filters</button></Empty>;
}
