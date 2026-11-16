import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { test } from "node:test";

// No DOM or renderer here: useNodeResource runs against a minimal hook
// runtime (state, refs, callbacks, effects for one component), and a fake
// `document` stands in for tab visibility.
type Slot = { value?: unknown; deps?: unknown[]; fn?: unknown; cleanup?: (() => void) | void };
type Hooks = { slots: Slot[]; i: number; effects: (() => void)[]; dirty: boolean; rerender: () => void };
const runtime = globalThis as unknown as { __hooks: Hooks };

const FAKE_REACT = `
const h = () => globalThis.__hooks;
const same = (a, b) => !!a && !!b && a.length === b.length && a.every((v, i) => Object.is(v, b[i]));
export function useState(init) {
  const s = h(), i = s.i++;
  s.slots[i] ??= { value: typeof init === "function" ? init() : init };
  const slot = s.slots[i];
  return [slot.value, next => { slot.value = typeof next === "function" ? next(slot.value) : next; s.dirty = true; s.rerender(); }];
}
export function useRef(init) { const s = h(), i = s.i++; s.slots[i] ??= { value: { current: init } }; return s.slots[i].value; }
export function useCallback(fn, deps) {
  const s = h(), i = s.i++, slot = s.slots[i];
  if (slot && same(slot.deps, deps)) return slot.fn;
  s.slots[i] = { fn, deps };
  return fn;
}
export function useEffect(fn, deps) {
  const s = h(), i = s.i++, slot = s.slots[i] ?? (s.slots[i] = {});
  if (slot.ran && same(slot.deps, deps)) return;
  slot.deps = deps; slot.ran = true;
  s.effects.push(() => { if (typeof slot.cleanup === "function") slot.cleanup(); slot.cleanup = fn(); });
}`;

registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL?.endsWith("/useNodeResource.ts")) {
      if (specifier === "react") return { url: "fake:react", shortCircuit: true };
      if (specifier === "./catalog") return { url: "fake:catalog", shortCircuit: true };
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === "fake:react") return { format: "module", source: FAKE_REACT, shortCircuit: true };
    if (url === "fake:catalog") return { format: "module", source: "export const afterShopPair = () => () => {};", shortCircuit: true };
    return next(url, context);
  },
});
const { useNodeResource } = await import("./useNodeResource.ts");

type Resource<T> = ReturnType<typeof useNodeResource<T>>;
function mount<T>(hook: () => Resource<T>) {
  const renders: Resource<T>[] = [];
  const s: Hooks = { slots: [], i: 0, effects: [], dirty: false, rerender: () => {} };
  const render = () => {
    runtime.__hooks = s;
    s.i = 0;
    s.effects = [];
    renders.push(hook());
    for (const effect of s.effects) effect();
  };
  s.rerender = () => queueMicrotask(() => { if (s.dirty) { s.dirty = false; render(); } });
  render();
  return {
    get current() { return renders[renders.length - 1]; },
    renders,
    unmount() { for (const slot of s.slots) if (typeof slot?.cleanup === "function") slot.cleanup(); },
  };
}

function fakeDocument() {
  const listeners = new Set<() => void>();
  const doc = {
    visibilityState: "visible" as "visible" | "hidden",
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    set(state: "visible" | "hidden") { doc.visibilityState = state; for (const fn of listeners) fn(); },
  };
  (globalThis as { document?: unknown }).document = doc;
  return doc;
}
const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

test("polls on the interval and keeps the last data without flashing a loading state", async () => {
  fakeDocument();
  let calls = 0;
  const view = mount(() => useNodeResource(async () => ++calls, 20));
  await sleep(110);
  view.unmount();
  assert.ok(calls >= 3, `polled ${calls} times in 110ms at a 20ms interval`);
  assert.equal(view.current.data, calls);
  const firstData = view.renders.findIndex(render => render.data !== null);
  assert.ok(view.renders.slice(firstData).every(render => !render.loading), "loading stays off after the first answer");
});

test("pauses while the tab is hidden and refreshes at once when it returns", async () => {
  const doc = fakeDocument();
  let calls = 0;
  const view = mount(() => useNodeResource(async () => ++calls, 20));
  await sleep(50);
  doc.set("hidden");
  await sleep(10);
  const hiddenAt = calls;
  await sleep(100);
  assert.equal(calls, hiddenAt, "no requests while hidden");
  doc.set("visible");
  await sleep(5);
  assert.equal(calls, hiddenAt + 1, "one request on return");
  await sleep(50);
  view.unmount();
  assert.ok(calls > hiddenAt + 1, "polling resumes");
});

test("never runs two requests at once, even with manual refreshes during a slow poll", async () => {
  fakeDocument();
  let running = 0, most = 0, calls = 0;
  const view = mount(() => useNodeResource(async () => {
    running++; calls++; most = Math.max(most, running);
    await sleep(30);
    running--;
    return calls;
  }, 5));
  await sleep(10);
  await Promise.all([view.current.refresh(), view.current.refresh(), view.current.refresh()]);
  await sleep(60);
  view.unmount();
  assert.equal(most, 1);
});

test("a failed poll keeps the last data and reports the error", async () => {
  fakeDocument();
  let calls = 0;
  const view = mount(() => useNodeResource(async () => { if (++calls > 1) throw new Error("node offline"); return "balance"; }, 15));
  await sleep(50);
  view.unmount();
  assert.equal(view.current.data, "balance");
  assert.equal(view.current.error, "node offline");
});
