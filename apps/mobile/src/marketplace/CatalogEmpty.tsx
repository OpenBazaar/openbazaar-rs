import { Empty, Icon } from "./Marketplace";

type Props = {
  section: string;
  query: string;
  category: string | null;
  filterCount: number;
  publicMode: boolean;
  onionNote: string | null;
  direct: string | null;
  onResetFilters: () => void;
  onDiscover: () => void;
  onRetry: () => void;
};

/** Empty-catalog state, separating active search/filter misses from an empty market or store. */
export function CatalogEmpty({
  section,
  query,
  category,
  filterCount,
  publicMode,
  onionNote,
  direct,
  onResetFilters,
  onDiscover,
  onRetry,
}: Props) {
  const narrowed = Boolean(query || category || filterCount > 0);
  if (section === "saved" && !narrowed) {
    return <Empty illustration="saved" icon="heart" title="No saved products" text="Save products using the heart button.">
      <button className="ob-secondary" onClick={onDiscover}>Show all products <Icon name="arrow" size={16}/></button>
    </Empty>;
  }
  if (narrowed) {
    return <Empty icon={section === "saved" ? "heart" : "search"} title="No matching products" text="Try a different search or explore another category.">
      <button className="ob-secondary" onClick={onResetFilters}>Show all products <Icon name="arrow" size={16}/></button>
    </Empty>;
  }
  if (publicMode && onionNote) {
    return <Empty icon="globe" title="Open in Tor Browser" text={onionNote}/>;
  }
  if (publicMode) {
    return <Empty icon="globe" title="No stores in the market yet" text="No admitted stores are published right now. Try again shortly, or open a store link directly.">
      <button className="ob-secondary" onClick={onRetry}>Try again <Icon name="arrow" size={16}/></button>
    </Empty>;
  }
  if (direct) {
    return <Empty icon="grid" title="No listings yet" text="This store has not published any listings yet."/>;
  }
  return <Empty icon="grid" title="No products yet" text="Follow a store or add a listing to see products here."/>;
}
