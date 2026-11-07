// Edits made while the node is out of reach, sent later in one
// POST /remote/batch. A batch keeps its id until its answer arrives, so a
// resend after a lost answer gets the node's stored results instead of
// running the edits twice. Pure: no storage, no network.

export const MAX_BATCH = 16;

export type Mutation = { method: "POST" | "PUT" | "PATCH" | "DELETE"; path: string; body?: unknown; label: string; queuedAt: number };
export type Batch = { id: string; requests: Mutation[] };
export type QueueState = { pending: Mutation[]; inFlight: Batch | null };
export type BatchResult = { status: number; body: unknown };
export type Outcome = { mutation: Mutation; result: BatchResult };

export function emptyQueue(): QueueState {
  return { pending: [], inFlight: null };
}

export function enqueue(state: QueueState, mutation: Mutation): QueueState {
  return { ...state, pending: [...state.pending, mutation] };
}

export function queuedCount(state: QueueState): number {
  return state.pending.length + (state.inFlight?.requests.length ?? 0);
}

/** The batch to send: the one still waiting for its answer, or up to 16 pending edits under a fresh id. */
export function nextBatch(state: QueueState, newId: () => string): { state: QueueState; batch: Batch | null } {
  if (state.inFlight) return { state, batch: state.inFlight };
  if (!state.pending.length) return { state, batch: null };
  const batch = { id: newId(), requests: state.pending.slice(0, MAX_BATCH) };
  return { state: { pending: state.pending.slice(MAX_BATCH), inFlight: batch }, batch };
}

export function batchBody(batch: Batch): { id: string; requests: { method: string; path: string; body?: unknown }[] } {
  return {
    id: batch.id,
    requests: batch.requests.map(({ method, path, body }) => (body === undefined ? { method, path } : { method, path, body })),
  };
}

/**
 * Settle the in-flight batch with the node's answer. 2xx are done, 4xx are
 * refused (retrying will not help), and an answer of 500 or above stops the
 * batch: it and every edit after it go back to the front of the queue.
 */
export function settleBatch(state: QueueState, answer: unknown): { state: QueueState; done: Outcome[]; refused: Outcome[] } {
  const value = answer && typeof answer === "object" ? (answer as { id?: unknown; results?: unknown }) : {};
  const batch = state.inFlight;
  if (!batch || value.id !== batch.id || !Array.isArray(value.results)) return { state, done: [], refused: [] };
  const done: Outcome[] = [];
  const refused: Outcome[] = [];
  let retry = batch.requests.length;
  for (const [i, raw] of (value.results as unknown[]).slice(0, batch.requests.length).entries()) {
    const entry = raw && typeof raw === "object" ? (raw as { status?: unknown; body?: unknown }) : {};
    const result = { status: typeof entry.status === "number" ? entry.status : 0, body: entry.body ?? null };
    const outcome = { mutation: batch.requests[i], result };
    if (result.status >= 500 || result.status === 0) {
      retry = i;
      break;
    }
    (result.status < 400 ? done : refused).push(outcome);
    retry = i + 1;
  }
  return { state: { pending: [...batch.requests.slice(retry), ...state.pending], inFlight: null }, done, refused };
}
