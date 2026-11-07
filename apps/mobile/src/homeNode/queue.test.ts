import assert from "node:assert/strict";
import { test } from "node:test";

import { batchBody, emptyQueue, enqueue, MAX_BATCH, nextBatch, queuedCount, settleBatch, type Mutation, type QueueState } from "./queue.ts";

const edit = (n: number): Mutation => ({ method: "POST", path: "/content-labels", body: { target: `h${n}`, rating: "general", tags: [] }, label: `Relabel ${n}`, queuedAt: n });

function queued(count: number): QueueState {
  let state = emptyQueue();
  for (let i = 0; i < count; i++) state = enqueue(state, edit(i));
  return state;
}

let ids = 0;
const newId = () => `batch${++ids}`;

test("edits wait in order and go out at most 16 at a time", () => {
  const { state, batch } = nextBatch(queued(20), newId);
  assert.equal(batch?.requests.length, MAX_BATCH);
  assert.equal(state.pending.length, 4);
  assert.equal(queuedCount(state), 20);
  assert.deepEqual(batchBody(batch!).requests[0], { method: "POST", path: "/content-labels", body: { target: "h0", rating: "general", tags: [] } });
  assert.deepEqual(nextBatch(emptyQueue(), newId), { state: emptyQueue(), batch: null });
});

test("a batch whose answer was lost is resent with the same id", () => {
  const sent = nextBatch(queued(2), newId);
  const again = nextBatch(enqueue(sent.state, edit(9)), newId);
  assert.equal(again.batch?.id, sent.batch?.id);
  assert.deepEqual(again.batch?.requests, sent.batch?.requests);
  const answer = { id: sent.batch!.id, results: [{ status: 200, body: {} }, { status: 200, body: {} }] };
  const settled = settleBatch(again.state, answer);
  assert.equal(settled.done.length, 2);
  assert.deepEqual(settled.state, { pending: [edit(9)], inFlight: null });
  assert.deepEqual(settleBatch(settled.state, answer).state, settled.state);
});

test("refused edits are dropped; a server error and what follows it go back to the front", () => {
  const sent = nextBatch(queued(4), newId);
  const settled = settleBatch(enqueue(sent.state, edit(7)), {
    id: sent.batch!.id,
    results: [{ status: 200, body: {} }, { status: 403, body: { code: "node_only" } }, { status: 502, body: {} }],
  });
  assert.deepEqual(settled.done.map(o => o.mutation), [edit(0)]);
  assert.deepEqual(settled.refused.map(o => o.mutation), [edit(1)]);
  assert.deepEqual(settled.state.pending, [edit(2), edit(3), edit(7)]);
  assert.equal(settled.state.inFlight, null);
});

test("an answer for another batch is ignored", () => {
  const sent = nextBatch(queued(1), newId);
  const settled = settleBatch(sent.state, { id: "other", results: [{ status: 200 }] });
  assert.deepEqual(settled.state, sent.state);
});
