import assert from "node:assert/strict";
import { test } from "node:test";

import { parseTorHealth, torWarning } from "./torHealth.ts";

const when = (secs: number) => `t${secs}`;
/// What a node sends: more than the page reads.
const sent = (state: string, reason: string) => ({ state, reason, since: 1794171605, checked: 1794204000, connected: true, onions: [{ role: "store", published: true }] });

test("a cut-off node says what is wrong, since when, and to restart it", () => {
  const reason = "The node's copy of the Tor directory expired 3 h 0 min ago and has not been replaced, so its onion addresses are probably unreachable.";
  assert.equal(
    torWarning(parseTorHealth(sent("cut off", reason)), when),
    `This node has been cut off from Tor since t1794171605. ${reason} Devices away from home cannot reach it. Check the node's internet connection, then restart the node.`,
  );
});

test("a degraded node says what is wrong and when to restart it", () => {
  const reason = "The store onion address has not been published on Tor for 15 min, so it may be unreachable.";
  assert.equal(
    torWarning(parseTorHealth(sent("degraded", reason)), when),
    `This node has had trouble with Tor since t1794171605. ${reason} It often recovers by itself. If it has not within an hour, restart the node.`,
  );
});

test("there is no warning while Tor is fine or starting", () => {
  assert.equal(torWarning(parseTorHealth(sent("fine", "Tor is connected.")), when), null);
  assert.equal(torWarning(parseTorHealth(sent("starting", "Tor is still starting.")), when), null);
});

test("an older node, a node without Tor, and a state this page does not know give no warning", () => {
  for (const value of [undefined, null, "cut off", 7, [], {}, { state: "cut off", reason: "?" }, { state: "on fire", reason: "?", since: 1 }, { state: "degraded", reason: 5, since: 1 }]) {
    assert.equal(parseTorHealth(value), null, JSON.stringify(value));
    assert.equal(torWarning(parseTorHealth(value), when), null);
  }
});
