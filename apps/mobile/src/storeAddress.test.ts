import assert from "node:assert/strict";
import { test } from "node:test";

import { followOutcome, followProblem, ownAddress, refusal, storeAddress } from "./storeAddress.ts";

const onion = `${"a2".repeat(28)}.onion`;
const row = { url: `http://${onion}`, storeId: "aa".repeat(32), name: "Cedar Workshop", listings: 2, fetched: 1, error: null };

test("a pasted store address becomes the URL the node follows", () => {
  for (const pasted of [`http://${onion}`, ` http://${onion}/ `, onion, onion.toUpperCase(), `HTTP://${onion}:80`]) assert.equal(storeAddress(pasted), `http://${onion}`);
  assert.equal(storeAddress("http://127.0.0.1:8788/"), "http://127.0.0.1:8788");
  assert.equal(storeAddress("localhost:8788"), "http://localhost:8788");
  assert.equal(storeAddress("http://[::1]:8788"), "http://[::1]:8788");
});

test("a clearnet host, a LAN address, https, a path, and a store key are not store addresses", () => {
  for (const pasted of ["", "http://shop.example", "http://192.168.1.20:8787", `https://${onion}`, `http://${onion}/public/store`, `http://${onion}:8080`, "http://127.0.0.1", "http://short.onion", `http://user@${onion}`, "aa".repeat(32)]) assert.equal(storeAddress(pasted), null, pasted);
});

test("a follow that fetched the store is named, and one that did not says why in plain words", () => {
  assert.deepEqual(followOutcome([row], row.url), { ok: true, message: "Following Cedar Workshop. Its listings are in Products." });
  const failed = (error: string) => followOutcome([{ ...row, storeId: null, name: null, listings: 0, error }], row.url);
  assert.deepEqual(failed("that node is not selling"), { ok: false, message: "That node answered, but it has no open store yet. Your node keeps the address and checks again." });
  assert.equal(failed("This node is not on Tor. Start it with --remote so a sale to an onion stays on Tor.").message, "Your node is not on Tor, so it cannot reach an onion address. Start it with --remote.");
  for (const unreachable of ["http://127.0.0.1:9/public/store: Connection Failed: Connect error: Connection refused (os error 61)", "the home node did not answer over Tor in time", "Network Error: timed out reading response"]) assert.equal(failed(unreachable).message, "Your node could not reach that store. It may be offline. Your node keeps the address and tries again.");
  assert.equal(failed("http://127.0.0.1:9: 500").message, "Your node could not copy that store yet. It keeps the address and tries again. The node said: http://127.0.0.1:9: 500");
  // An older copy does not make a failed fetch a success, and a list without the row is not one either.
  assert.equal(followOutcome([{ ...row, error: "Connection refused" }], row.url).ok, false);
  assert.equal(followOutcome([], row.url).ok, false);
  assert.equal(followProblem("the store sent a listing it did not sign").endsWith("The node said: the store sent a listing it did not sign"), true);
});

test("a name the node says is no onion is called a bad address, and any other refusal is the node's own sentence", () => {
  assert.equal(refusal(`http://${onion} is not a loopback address or a Tor onion`), "That is not a real store address. Check that it was copied whole.");
  assert.equal(refusal("That is this node's own address."), "That is this node's own address.");
  assert.equal(refusal("This node already follows the most stores it keeps."), "This node already follows the most stores it keeps.");
});

test("the vendor's shareable address is the store onion, and a loopback address is only local", () => {
  const tor = { state: "fine", reason: "", since: 1 };
  assert.deepEqual(ownAddress({ enabled: true, saleOnion: onion, tor }, "http://127.0.0.1:8787"), { onion: `http://${onion}`, onTor: true, local: "http://127.0.0.1:8787" });
  assert.deepEqual(ownAddress({ enabled: true, tor }, "http://localhost:8787"), { onion: null, onTor: true, local: "http://localhost:8787" });
  assert.deepEqual(ownAddress({ enabled: false }, "http://127.0.0.1:8787"), { onion: null, onTor: false, local: "http://127.0.0.1:8787" });
  // A page served by a home node sits at that node's own onion or web address: neither is a store address.
  assert.deepEqual(ownAddress({ saleOnion: "http://127.0.0.1:9" }, `http://${onion}`), { onion: null, onTor: false, local: null });
  assert.deepEqual(ownAddress(null, "https://node.example"), { onion: null, onTor: false, local: null });
});
