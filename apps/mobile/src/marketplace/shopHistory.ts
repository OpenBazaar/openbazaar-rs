import type { CheckoutReceipt, ListingHeader } from "../catalog";
import type { CartLine } from "../cart";

// One browser history stack for the shop. The on-screen back button calls
// history.back(), and popstate applies that same entry. A page is pushed only
// when the current entry is the page its back button returns to, so the mouse
// button and the on-screen button cannot drift apart.

export type ShopPage =
  | { name: "browse" }
  | { name: "store"; storeId: string }
  | { name: "detail"; listing: ListingHeader; fromStore?: string }
  | { name: "checkout"; lines: CartLine[]; returnTo?: ListingHeader; fromStore?: string }
  | { name: "receipt"; receipt: CheckoutReceipt };

export type ShopLocation = {
  section: string;
  category: string | null;
  page: ShopPage;
};

type Frame = { depth: number; loc: ShopLocation };
type Stamp = { ob: "shop"; i: number; depth: number; loc: ShopLocation };

export type HistoryLike = {
  readonly state: unknown;
  pushState(data: unknown, unused: string): void;
  replaceState(data: unknown, unused: string): void;
  back(): void;
  go(delta: number): void;
  scrollRestoration?: "auto" | "manual";
};

const MARK = "shop";

function stamp(i: number, depth: number, loc: ShopLocation): Stamp {
  return { ob: MARK, i, depth, loc };
}

function parse(state: unknown): Stamp | null {
  if (!state || typeof state !== "object") return null;
  const value = state as Partial<Stamp>;
  const index = value.i;
  const depth = value.depth;
  if (value.ob !== MARK || typeof index !== "number" || !Number.isInteger(index) || index < 0 || typeof depth !== "number" || !Number.isInteger(depth) || depth < 0) return null;
  const loc = value.loc;
  if (!loc || typeof loc !== "object" || typeof loc.section !== "string") return null;
  if (loc.category !== null && typeof loc.category !== "string") return null;
  if (!knownPage(loc.page)) return null;
  return { ob: MARK, i: index, depth, loc };
}

function knownPage(page: ShopPage | undefined): page is ShopPage {
  if (!page || typeof page !== "object") return false;
  if (page.name === "browse") return true;
  if (page.name === "store") return typeof page.storeId === "string";
  if (page.name === "detail") return !!page.listing && typeof page.listing.hash === "string";
  if (page.name === "checkout") return Array.isArray(page.lines);
  if (page.name === "receipt") return !!page.receipt && typeof page.receipt.checkoutId === "string";
  return false;
}

function samePage(a: ShopPage, b: ShopPage): boolean {
  if (a.name !== b.name) return false;
  if (a.name === "browse" && b.name === "browse") return true;
  if (a.name === "store" && b.name === "store") return a.storeId === b.storeId;
  if (a.name === "detail" && b.name === "detail") return a.listing.hash === b.listing.hash && (a.fromStore ?? "") === (b.fromStore ?? "");
  if (a.name === "checkout" && b.name === "checkout") {
    return (a.fromStore ?? "") === (b.fromStore ?? "")
      && (a.returnTo?.hash ?? "") === (b.returnTo?.hash ?? "")
      && a.lines.length === b.lines.length
      && a.lines.every((line, i) => line.hash === b.lines[i]?.hash && line.quantity === b.lines[i]?.quantity && (line.variant ?? "") === (b.lines[i]?.variant ?? ""));
  }
  if (a.name === "receipt" && b.name === "receipt") return a.receipt.checkoutId === b.receipt.checkoutId;
  return false;
}

function same(a: ShopLocation, b: ShopLocation): boolean {
  return a.section === b.section && a.category === b.category && samePage(a.page, b.page);
}

/** Where the on-screen back button goes, or null when this screen has none. */
function backTarget(loc: ShopLocation): ShopLocation | null {
  const page = loc.page;
  const rest = { section: loc.section, category: loc.category };
  if (page.name === "store") return { ...rest, page: { name: "browse" } };
  if (page.name === "detail") {
    return page.fromStore
      ? { ...rest, page: { name: "store", storeId: page.fromStore } }
      : { ...rest, page: { name: "browse" } };
  }
  if (page.name === "checkout") {
    if (page.returnTo) {
      return {
        ...rest,
        page: page.fromStore
          ? { name: "detail", listing: page.returnTo, fromStore: page.fromStore }
          : { name: "detail", listing: page.returnTo },
      };
    }
    return { section: "bag", category: null, page: { name: "browse" } };
  }
  return null;
}

function pageDepth(loc: ShopLocation): number {
  const page = loc.page;
  if (page.name === "store") return 1;
  if (page.name === "detail") return page.fromStore ? 2 : 1;
  if (page.name === "checkout") {
    if (!page.returnTo) return 1;
    return page.fromStore ? 3 : 2;
  }
  return 0;
}

function browserHistory(): HistoryLike {
  if (typeof window === "undefined") {
    return { get state() { return null; }, pushState() {}, replaceState() {}, back() {}, go() {} };
  }
  return window.history;
}

export class ShopHistory {
  private stack: Frame[] = [];
  private cursor = 0;
  private pending: { loc: ShopLocation; depth: number } | null = null;
  private apply: ((loc: ShopLocation) => void) | undefined;
  private history: HistoryLike;

  constructor(history: HistoryLike = browserHistory()) {
    this.history = history;
  }

  /** Stamp the current document as the shop root, or keep a restored entry. */
  start(initial: ShopLocation): ShopLocation {
    if (this.history.scrollRestoration) this.history.scrollRestoration = "manual";
    const existing = parse(this.history.state);
    if (existing && this.stack[existing.i]) {
      this.cursor = existing.i;
      return this.stack[existing.i].loc;
    }
    if (existing) {
      this.cursor = existing.i;
      this.stack[existing.i] = { depth: existing.depth, loc: existing.loc };
      return existing.loc;
    }
    const frame = { depth: 0, loc: initial };
    this.stack = [frame];
    this.cursor = 0;
    this.history.replaceState(stamp(0, 0, initial), "");
    return initial;
  }

  listen(apply: (loc: ShopLocation) => void): () => void {
    this.apply = apply;
    if (typeof window === "undefined") return () => { if (this.apply === apply) this.apply = undefined; };
    const onPop = (event: PopStateEvent) => this.onPop(event.state);
    window.addEventListener("popstate", onPop);
    return () => {
      if (this.apply === apply) this.apply = undefined;
      window.removeEventListener("popstate", onPop);
    };
  }

  /** Keep an in-page change (a category chip) on the current entry. */
  note(loc: ShopLocation) {
    if (this.pending) return;
    const cur = this.current();
    if (!cur || same(cur.loc, loc)) return;
    if (cur.loc.section !== loc.section || !samePage(cur.loc.page, loc.page)) return;
    this.history.replaceState(stamp(this.cursor, cur.depth, loc), "");
    cur.loc = loc;
  }

  /** A sidebar section. Pushed so back returns here only when a page sits under it. */
  section(next: ShopLocation) {
    if (this.pending) return;
    const cur = this.current();
    if (cur && same(cur.loc, next)) return;
    this.push(next, 0);
  }

  /** Open a store, product, or checkout. The entry underneath is that screen's back target. */
  page(next: ShopLocation) {
    if (this.pending) return;
    this.syncCategory(next.section, next.category);
    const cur = this.current();
    if (cur && same(cur.loc, next)) return;
    const parent = backTarget(next);
    const depth = pageDepth(next);
    if (!parent || (cur && same(cur.loc, parent))) {
      this.push(next, parent ? depth : 0);
      return;
    }
    const prev = this.cursor > 0 ? this.stack[this.cursor - 1] : undefined;
    if (prev && same(prev.loc, next)) {
      this.history.back();
      return;
    }
    if (prev && same(prev.loc, parent)) {
      this.write(this.cursor, depth, next, "replace");
      this.apply?.(next);
      return;
    }
    const steps = cur?.depth ?? 0;
    if (steps > 0) {
      this.pending = { loc: next, depth };
      this.history.go(-steps);
      return;
    }
    this.push(next, depth);
  }

  /** A finished checkout replaces itself so back does not return to it. */
  replace(next: ShopLocation) {
    if (this.pending) return;
    const cur = this.current();
    if (!cur) {
      this.push(next, 0);
      return;
    }
    this.write(this.cursor, cur.depth, next, "replace");
    this.apply?.(next);
  }

  /** The on-screen back button. popstate applies the entry this reveals. */
  back() {
    this.history.back();
  }

  /** Leave a product for the browse page under this visit (hide listing). */
  backToBrowse() {
    const cur = this.current();
    if (!cur || cur.depth === 0) {
      if (cur && cur.loc.page.name !== "browse") this.push({ section: cur.loc.section, category: cur.loc.category, page: { name: "browse" } }, 0);
      return;
    }
    this.history.go(-cur.depth);
  }

  onPop(state: unknown) {
    const parsed = parse(state);
    if (this.pending) {
      const next = this.pending;
      this.pending = null;
      if (!parsed) return;
      this.cursor = parsed.i;
      this.stack.length = parsed.i + 1;
      this.stack[parsed.i] = { depth: parsed.depth, loc: parsed.loc };
      this.push(next.loc, next.depth);
      return;
    }
    if (!parsed) return;
    this.cursor = parsed.i;
    this.stack[parsed.i] = { depth: parsed.depth, loc: parsed.loc };
    this.apply?.(parsed.loc);
  }

  private current(): Frame | undefined {
    return this.stack[this.cursor];
  }

  private syncCategory(section: string, category: string | null) {
    const cur = this.current();
    if (!cur || cur.loc.section !== section || cur.loc.category === category) return;
    const loc = { section: cur.loc.section, category, page: cur.loc.page };
    this.history.replaceState(stamp(this.cursor, cur.depth, loc), "");
    cur.loc = loc;
  }

  private push(loc: ShopLocation, depth: number) {
    this.write(this.cursor + 1, depth, loc, "push");
    this.apply?.(loc);
  }

  private write(index: number, depth: number, loc: ShopLocation, mode: "push" | "replace") {
    const state = stamp(index, depth, loc);
    if (mode === "push") this.history.pushState(state, "");
    else this.history.replaceState(state, "");
    if (mode === "push") this.stack.length = index;
    this.stack[index] = { depth, loc };
    this.cursor = index;
  }
}

export const shopHistory = new ShopHistory();
