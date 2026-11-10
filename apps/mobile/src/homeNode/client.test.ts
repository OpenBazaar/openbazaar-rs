import assert from "node:assert/strict";
import { test } from "node:test";
import { chacha20poly1305, xchacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { p256 } from "@noble/curves/nist.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { AnswerLostError, HomeNodeClient, UnknownDeviceError, type Channels, type NodeLink } from "./client.ts";
import { deviceIdOf, nodeIdOf, pairingOnion, requestPreimage, sas, transcript } from "./protocol.ts";
import { pollPairing, requestPairing } from "./pairing.ts";

const nodeSecret = new Uint8Array(32).fill(3);
const nodePublic = x25519.getPublicKey(nodeSecret);
const deviceSecret = new Uint8Array(32).fill(4);
const keyText = `p256:${bytesToHex(p256.getPublicKey(deviceSecret, true))}`;
const deviceId = deviceIdOf(keyText);
const onion = "b".repeat(56);
const link: NodeLink = { nodeId: nodeIdOf(nodePublic), nodeKey: bytesToHex(nodePublic), onion, lan: ["192.168.1.20:8790"], deviceId };
const signer = { deviceId, sign: async (preimage: string) => bytesToHex(p256.sign(utf8ToBytes(preimage), deviceSecret, { format: "der" })) };
const LABEL = utf8ToBytes("OBLAN1");
const RESPONSE_LABEL = utf8ToBytes("OBLAN2");

type Seen = { via: "lan" | "tor"; method: string; path: string; headers: Record<string, string>; body: string };

function parse(bytes: Uint8Array): Omit<Seen, "via"> {
  const text = new TextDecoder().decode(bytes);
  const [head, body] = text.split("\r\n\r\n");
  const [line, ...rest] = head.split("\r\n");
  const [method, path] = line.split(" ");
  const headers = Object.fromEntries(rest.map(item => [item.slice(0, item.indexOf(":")).toLowerCase(), item.slice(item.indexOf(":") + 1).trim()]));
  return { method, path, headers, body };
}

function answer(status: number, value: unknown): Uint8Array {
  const body = JSON.stringify(value);
  return utf8ToBytes(`HTTP/1.1 ${status} X\r\nContent-Type: application/json\r\nContent-Length: ${utf8ToBytes(body).length}\r\n\r\n${body}`);
}

/** A node that verifies signatures and answers on both channels. */
function fakeNode(handle: (seen: Seen) => Uint8Array, options: { lanUp?: boolean; dropLanAnswer?: boolean } = {}) {
  const seen: Seen[] = [];
  const serve = (via: "lan" | "tor", bytes: Uint8Array) => {
    const request = { via, ...parse(bytes) };
    seen.push(request);
    return handle(request);
  };
  const channels: Channels = {
    async tor(target, bytes) {
      assert.ok(target === onion || target.length === 56);
      return serve("tor", bytes);
    },
    async lan(address, body) {
      if (!options.lanUp) throw new Error("no route to host");
      assert.equal(address, "192.168.1.20:8790");
      const E = body.slice(0, 32);
      const K = hkdf(sha256, x25519.getSharedSecret(nodeSecret, E), LABEL, concatBytes(E, nodePublic), 64);
      const inner = chacha20poly1305(K.slice(0, 32), new Uint8Array(12), LABEL).decrypt(body.slice(32));
      const out = serve("lan", inner);
      if (options.dropLanAnswer && seen.at(-1)?.method !== "GET") throw new Error("connection reset");
      const nonce = randomBytes(24);
      return { status: 200, body: concatBytes(new Uint8Array([2]), nonce, xchacha20poly1305(K.slice(32), nonce, RESPONSE_LABEL).encrypt(out)) };
    },
  };
  return { seen, channels };
}

function verified(request: Seen): boolean {
  const preimage = requestPreimage(link.nodeId, request.headers["x-ob-device"], Number(request.headers["x-ob-time"]), request.headers["x-ob-nonce"], request.method, request.path, utf8ToBytes(request.body));
  return p256.verify(hexToBytes(request.headers["x-ob-signature"]), utf8ToBytes(preimage), hexToBytes(keyText.slice(5)), { format: "der" });
}

test("signed requests go over Tor when the LAN does not answer", async () => {
  const node = fakeNode(request => {
    assert.ok(verified(request));
    return answer(200, { ok: true });
  });
  const client = new HomeNodeClient(link, signer, node.channels, () => {});
  const result = await client.request("POST", "/remote/sync", utf8ToBytes('{"have":{}}'));
  assert.equal(result.status, 200);
  assert.equal(result.via, "tor");
  assert.equal(node.seen.length, 1);
  assert.equal(node.seen[0].headers.host, `${onion}.onion`);
  assert.equal(node.seen[0].headers["x-ob-device"], deviceId);
  assert.match(node.seen[0].headers["x-ob-nonce"], /^[0-9a-f]{32}$/);
});

test("the LAN channel is used when the node opens the probe", async () => {
  const node = fakeNode(request => (request.path === "/remote/hello" ? answer(200, {}) : answer(200, { via: request.via })), { lanUp: true });
  const client = new HomeNodeClient(link, signer, node.channels, () => {});
  const states: (string | null)[] = [];
  client.subscribe(state => states.push(state.via));
  const result = await client.request("GET", "/catalog");
  assert.equal(result.via, "lan");
  assert.deepEqual(node.seen.map(r => `${r.via} ${r.path}`), ["lan /remote/hello", "lan /catalog"]);
  assert.ok(verified(node.seen[1]));
  assert.equal(states.at(-1), "lan");
});

test("a mutation whose LAN answer was lost is not resent; a read falls back to Tor", async () => {
  const node = fakeNode(() => answer(200, {}), { lanUp: true, dropLanAnswer: true });
  const client = new HomeNodeClient(link, signer, node.channels, () => {});
  await assert.rejects(client.request("POST", "/content-labels", utf8ToBytes("{}")), AnswerLostError);
  assert.equal(node.seen.filter(r => r.path === "/content-labels").length, 1);
  const read = await client.request("GET", "/catalog");
  assert.equal(read.via, "tor");
});

test("an idempotent batch is resent over Tor after a lost LAN answer", async () => {
  const node = fakeNode(() => answer(200, {}), { lanUp: true, dropLanAnswer: true });
  const client = new HomeNodeClient(link, signer, node.channels, () => {});
  const result = await client.request("POST", "/remote/batch", utf8ToBytes('{"id":"x"}'), { idempotent: true });
  assert.equal(result.via, "tor");
});

test("a clock refusal corrects the offset and retries once", async () => {
  let calls = 0;
  const node = fakeNode(request => {
    calls++;
    if (Number(request.headers["x-ob-time"]) !== 5000) return answer(401, { code: "clock", serverTime: 5000 });
    return answer(200, {});
  });
  const client = new HomeNodeClient(link, signer, node.channels, () => {}, () => 1_000_000);
  assert.equal((await client.request("GET", "/version")).status, 200);
  assert.equal(calls, 2);
  assert.equal(client.now(), 5000);
  const stubborn = new HomeNodeClient(link, signer, fakeNode(() => answer(401, { code: "clock", serverTime: 1 })).channels, () => {});
  assert.equal((await stubborn.request("GET", "/version")).status, 401);
});

test("an image upload keeps the image content type", async () => {
  const png = Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0xff]);
  const node = fakeNode(request => {
    assert.equal(request.headers["content-type"], "image/png");
    assert.equal(request.headers["content-length"], String(png.length));
    return answer(200, { hash: "ok" });
  });
  const client = new HomeNodeClient(link, signer, node.channels, () => {});
  const result = await client.request("POST", "/media", png, { contentType: "image/png" });
  assert.equal(result.status, 200);
});

test("unknown_device forgets the node", async () => {
  let forgot = false;
  const node = fakeNode(() => answer(401, { code: "unknown_device" }));
  const client = new HomeNodeClient(link, signer, node.channels, () => (forgot = true));
  await assert.rejects(client.request("GET", "/catalog"), UnknownDeviceError);
  assert.ok(forgot);
});

test("pairing over the derived onion, then approval", async () => {
  const secret = hexToBytes("0a0b0c0d0e0f10111213");
  const node = fakeNode(request => {
    if (request.path === "/pair/request") {
      const body = JSON.parse(request.body);
      const text = transcript(body);
      assert.equal(body.proof, bytesToHex(hmac(sha256, secret, utf8ToBytes(text))));
      assert.ok(p256.verify(hexToBytes(body.signature), utf8ToBytes(text), hexToBytes(body.key.slice(5)), { format: "der" }));
      assert.equal(request.headers.host, `${pairingOnion(secret)}.onion`);
      const pictures = sas(secret, nodePublic, text);
      return answer(202, { requestId: "r".repeat(32), nodeKey: bytesToHex(nodePublic), sas: pictures.map(p => p.emoji), sasNames: pictures.map(p => p.name), expiresInSecs: 600 });
    }
    assert.equal(request.path, `/pair/status/${"r".repeat(32)}`);
    return answer(200, { state: "approved", deviceId, nodeId: link.nodeId, onion: `${onion}.onion`, lan: ["192.168.1.20:8790"] });
  });
  const request = { key: keyText, name: "Test phone", platform: "ios" as const, sign: async (text: string) => signer.sign(text) };
  const pending = await requestPairing(node.channels, { secret, lan: [] }, request);
  assert.equal(pending.route.via, "tor");
  assert.equal(pending.sas.length, 4);
  const result = await pollPairing(node.channels, pending, keyText);
  assert.deepEqual(result, { state: "approved", link, discoverySecret: null });
});

test("pairing keeps asking a pairing onion that Tor cannot reach yet", async () => {
  const secret = hexToBytes("0a0b0c0d0e0f10111213");
  const request = { key: keyText, name: "Test phone", platform: "ios" as const, sign: async (text: string) => signer.sign(text) };
  const pictures = sas(secret, nodePublic, transcript(request));
  const accepted = answer(202, { requestId: "r".repeat(32), nodeKey: bytesToHex(nodePublic), sasNames: pictures.map(p => p.name) });
  // The first two attempts end the way Arti ends them while a new onion is not reachable.
  const failures = ["the home node did not answer over Tor in time", "tor: Onion Service not found"];
  let dials = 0;
  const channels: Channels = {
    async tor(target) {
      assert.equal(target, pairingOnion(secret));
      dials++;
      const failure = failures.shift();
      if (failure) throw new Error(failure);
      return accepted;
    },
    async lan() {
      throw new Error("no route to host");
    },
  };
  const reasons: string[] = [];
  const pending = await requestPairing(channels, { secret, lan: [] }, request, { forMs: 60_000, pauseMs: 0, retrying: reason => reasons.push(reason) });
  assert.equal(pending.requestId, "r".repeat(32));
  assert.equal(dials, 3);
  assert.deepEqual(reasons, ["the home node did not answer over Tor in time", "tor: Onion Service not found"]);

  // When the patience runs out, the last reason is in the message.
  failures.push("the home node did not answer over Tor in time");
  await assert.rejects(requestPairing(channels, { secret, lan: [] }, request, { forMs: 0, pauseMs: 0 }), /did not answer over Tor \(the home node did not answer over Tor in time\)/);
  assert.equal(dials, 4);

  // An answer from the node is final, whatever it says.
  const refusing: Channels = { ...channels, tor: async () => { dials++; return answer(409, { error: "taken", code: "code_taken" }); } };
  await assert.rejects(requestPairing(refusing, { secret, lan: [] }, request, { forMs: 60_000, pauseMs: 0 }), /Another device already used this code/);
  assert.equal(dials, 5);
});

test("pairing refuses a node key that differs from the QR code", async () => {
  const node = fakeNode(() => answer(202, { requestId: "r".repeat(32), nodeKey: "00".repeat(32) }));
  const request = { key: keyText, name: "Test phone", platform: "ios" as const, sign: async () => "00" };
  await assert.rejects(requestPairing(node.channels, { secret: new Uint8Array(10), nodePublic, onion, lan: [] }, request), /not the one in the QR code/);
});
