import { useSyncExternalStore } from "react";
import { bytesToHex, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { deleteKey, signWith } from "../../modules/ob-device-key";
import { HomeNodeClient, jsonBody, jsonOf, UnknownDeviceError, type Signer, type Via } from "../homeNode/client.ts";
import { deviceIdOf } from "../homeNode/protocol.ts";
import { batchBody, emptyQueue, enqueue, nextBatch, settleBatch, type Mutation, type Outcome, type QueueState } from "../homeNode/queue.ts";
import { applySync, emptyReplica, haveOf, type Replica, type SyncChanges } from "../homeNode/replica.ts";
import { setNativeHomeFetch } from "../homeNode/transport.ts";
import { channels, ensureTor, insertDiscoveryKey, torStatus } from "./channels.ts";
import { forgetEverything, loadPairing, readSealed, savePairing, writeSealed, type Pairing } from "./storage.ts";

// The paired node as the app sees it: one client, the replica, the queue of
// edits, and the connection badge. Screens read it with useSession().

export const DEVICE_ALIAS = "ob-device";
export const WATCH_ALIAS = "ob-watch";

export type SessionState = {
  status: "loading" | "unpaired" | "paired";
  pairing: Pairing | null;
  via: Via | null;
  inFlight: number;
  torReady: boolean;
  replica: Replica;
  queue: QueueState;
  syncing: boolean;
  lastSyncAt: number | null;
  lastError: string | null;
  refused: Outcome[];
};

let state: SessionState = {
  status: "loading",
  pairing: null,
  via: null,
  inFlight: 0,
  torReady: false,
  replica: emptyReplica(),
  queue: emptyQueue(),
  syncing: false,
  lastSyncAt: null,
  lastError: null,
  refused: [],
};
const listeners = new Set<() => void>();
let client: HomeNodeClient | null = null;

function set(next: Partial<SessionState>) {
  state = { ...state, ...next };
  for (const listener of listeners) listener();
}

export function getSession(): SessionState {
  return state;
}

export function useSession(): SessionState {
  return useSyncExternalStore(
    listener => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
  );
}

export function signerFor(kind: "device" | "watch", keyText: string): Signer {
  return { deviceId: deviceIdOf(keyText), sign: preimage => signWith(kind === "device" ? DEVICE_ALIAS : WATCH_ALIAS, kind, preimage) };
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The client for the paired node, built once per pairing. */
export function homeClient(): HomeNodeClient {
  if (!client) throw new UnknownDeviceError();
  return client;
}

async function nativeFetch(path: string, init: RequestInit): Promise<Response> {
  const method = (init.method ?? "GET").toUpperCase();
  const body = typeof init.body === "string" ? utf8ToBytes(init.body) : new Uint8Array();
  const answer = await homeClient().request(method, path, body);
  return new Response(new TextDecoder().decode(answer.body), {
    status: answer.status,
    headers: { "Content-Type": answer.headers["content-type"] ?? "application/json" },
  });
}

/** Load the pairing, the replica, and the queue, and start Tor. Safe to call again. */
export async function boot(): Promise<void> {
  const pairing = await loadPairing();
  if (!pairing) {
    client = null;
    setNativeHomeFetch(null);
    set({ status: "unpaired", pairing: null });
    return;
  }
  client = new HomeNodeClient(pairing.link, signerFor("device", pairing.keyText), channels, () => void forget(false));
  client.subscribe(({ via, inFlight }) => set({ via, inFlight }));
  setNativeHomeFetch(nativeFetch);
  const replica = ((await readSealed("replica.obs")) as Replica | null) ?? emptyReplica();
  const queue = ((await readSealed("queue.obs")) as QueueState | null) ?? emptyQueue();
  set({ status: "paired", pairing, replica, queue });
  void startTorFor(pairing);
}

async function startTorFor(pairing: Pairing) {
  try {
    await ensureTor();
    await insertDiscoveryKey(pairing.link.onion, pairing.discoverySecret);
  } catch (err) {
    set({ lastError: errorText(err) });
  }
  const poll = setInterval(() => {
    const ready = torStatus().state === "ready";
    if (ready !== state.torReady) set({ torReady: ready });
    if (ready) clearInterval(poll);
  }, 1000);
}

async function updatePairing(next: Pairing) {
  await savePairing(next);
  if (client) client.link = next.link;
  set({ pairing: next });
}

/** Follow the node's reported LAN addresses and an announced address move. */
async function followNode(node: Record<string, unknown> | null) {
  const pairing = state.pairing;
  if (!node || !pairing) return;
  const lan = Array.isArray(node.lan) ? node.lan.filter((item): item is string => typeof item === "string") : pairing.link.lan;
  const next = typeof node.nextOnion === "string" ? node.nextOnion.toLowerCase().replace(/\.onion$/, "") : null;
  if (next && /^[a-z2-7]{56}$/.test(next) && next !== pairing.link.onion) {
    await insertDiscoveryKey(next, pairing.discoverySecret);
    await updatePairing({ ...pairing, link: { ...pairing.link, onion: next, lan } });
  } else if (lan.join() !== pairing.link.lan.join()) {
    await updatePairing({ ...pairing, link: { ...pairing.link, lan } });
  }
}

async function pull(signer?: Signer): Promise<SyncChanges> {
  const answer = await homeClient().request("POST", "/remote/sync", jsonBody({ have: haveOf(state.replica) }), { signer, idempotent: true });
  if (answer.status !== 200) throw new Error(String(jsonOf(answer.body).error ?? `The node answered ${answer.status}.`));
  const { replica, changes } = applySync(state.replica, JSON.parse(new TextDecoder().decode(answer.body)));
  await writeSealed("replica.obs", replica);
  set({ replica, lastSyncAt: Date.now(), lastError: null });
  await followNode(replica.node);
  return changes;
}

/** One POST /remote/sync, then queued edits, then a second sync if any landed. `watch` signs with the watch key (background). */
export async function sync(options: { watch?: boolean } = {}): Promise<SyncChanges | null> {
  const pairing = state.pairing;
  if (!client || !pairing || state.syncing) return null;
  set({ syncing: true });
  try {
    const signer = options.watch && pairing.watchKeyText ? signerFor("watch", pairing.watchKeyText) : undefined;
    const changes = await pull(signer);
    if (!options.watch && (await flush())) await pull();
    return changes;
  } catch (err) {
    if (!(err instanceof UnknownDeviceError)) set({ lastError: errorText(err) });
    return null;
  } finally {
    set({ syncing: false });
  }
}

/** Send queued edits as one batch; true if any landed. A lost answer leaves the batch in flight, and the next flush resends it under the same id. */
async function flush(): Promise<boolean> {
  const { state: queue, batch } = nextBatch(state.queue, () => bytesToHex(randomBytes(16)));
  if (!batch) return false;
  await writeSealed("queue.obs", queue);
  set({ queue });
  const answer = await homeClient().request("POST", "/remote/batch", jsonBody(batchBody(batch)), { idempotent: true });
  if (answer.status !== 200) return false;
  const settled = settleBatch(state.queue, JSON.parse(new TextDecoder().decode(answer.body)));
  await writeSealed("queue.obs", settled.state);
  set({ queue: settled.state, refused: [...state.refused, ...settled.refused] });
  return settled.done.length > 0;
}

/** Keep an edit in the queue and try to send it now; it waits there if the node is out of reach. */
export async function queueEdit(mutation: Omit<Mutation, "queuedAt">): Promise<void> {
  const queue = enqueue(state.queue, { ...mutation, queuedAt: Date.now() });
  await writeSealed("queue.obs", queue);
  set({ queue });
  void sync();
}

export function dismissRefused() {
  set({ refused: [] });
}

export async function setPushEnabled(enabled: boolean) {
  if (state.pairing) await updatePairing({ ...state.pairing, pushEnabled: enabled });
}

/** Drop the node and this phone's keys. With `tellNode`, ask the node to remove this device first. */
export async function forget(tellNode: boolean): Promise<void> {
  if (tellNode && client && state.pairing) {
    await client.request("DELETE", `/remote/devices/${state.pairing.link.deviceId}`).catch(() => null);
  }
  client = null;
  setNativeHomeFetch(null);
  await Promise.all([deleteKey(DEVICE_ALIAS), deleteKey(WATCH_ALIAS)]).catch(() => null);
  await forgetEverything();
  set({ status: "unpaired", pairing: null, replica: emptyReplica(), queue: emptyQueue(), via: null, lastError: null, refused: [] });
}
