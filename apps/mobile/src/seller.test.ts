import assert from "node:assert/strict";
import { test } from "node:test";

import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

import { parseBuyerOrder, showPairingWith } from "./catalog.ts";
import { bytesToHex, verifyListingRecord } from "./records.ts";
import { displayNameError, draftOf, formatPrice, fromMinorUnits, parseFollows, parseOwnListings, parseProfile, parseSale, salesToShip, storeImageChange, testNetworkSeeds, toMinorUnits, uploadImage } from "./seller.ts";

const SECRET = new Uint8Array(32).fill(7);
const STORE = bytesToHex(schnorr.getPublicKey(SECRET));
const ID = "0123456789abcdef0123456789abcdef";
const IMAGE = "ab".repeat(32);

/// A version 6 listing laid out as crates/protocol writes it, signed with SECRET.
function v6Listing(price: { currency: string; amount: number; shipping: number } | null): string {
  const out: number[] = [];
  const enc = new TextEncoder();
  const u = (value: number, bytes: number) => {
    for (let i = 0; i < bytes; i++) out.push(Math.floor(value / 256 ** i) % 256);
  };
  const str = (text: string) => {
    const b = enc.encode(text);
    u(b.length, 2);
    out.push(...b);
  };
  const raw = (hex: string) => out.push(...Array.from({ length: hex.length / 2 }, (_, i) => parseInt(hex.slice(i * 2, i * 2 + 2), 16)));
  out.push(...enc.encode("OBLS"), 6);
  raw(STORE);
  str("Stoneware mug");
  str("Home");
  u(25000, 8);
  str("US");
  str("Wheel thrown.");
  str("Brook and Pine");
  u(5, 4);
  u(20260107, 4);
  out.push(0, 0);
  str("");
  u(3000, 8);
  u(0, 8);
  out.push(1);
  u(0b10, 2);
  raw(ID);
  out.push(1);
  raw(IMAGE);
  if (price) {
    out.push(1);
    str(price.currency);
    u(price.amount, 8);
    u(price.shipping, 8);
  } else out.push(0);
  const body = Uint8Array.from(out);
  const tag = sha256(enc.encode("ob/listing/v1"));
  const digest = sha256(Uint8Array.from([...tag, ...tag, ...body]));
  return bytesToHex(Uint8Array.from([...body, ...schnorr.sign(digest, SECRET)]));
}

test("a version 6 listing verifies with its id, images, and fiat price", () => {
  const record = verifyListingRecord(v6Listing({ currency: "USD", amount: 1250, shipping: 300 }));
  assert.ok(record);
  assert.equal(record.version, 6);
  assert.equal(record.listingId, ID);
  assert.deepEqual(record.images, [IMAGE]);
  assert.deepEqual(record.price, { currency: "USD", amount: 1250, shipping: 300 });
  assert.equal(record.contentRating, "sensitive");
  assert.deepEqual(record.contentTags, ["violence"]);
  const sats = verifyListingRecord(v6Listing(null));
  assert.equal(sats?.price, null);
});

test("a changed version 6 byte fails", () => {
  const hex = v6Listing(null);
  const flipped = hex.slice(0, hex.length - 140) + (hex[hex.length - 140] === "0" ? "1" : "0") + hex.slice(hex.length - 139);
  assert.equal(verifyListingRecord(flipped), null);
});

test("own listings keep only records the own store signed, and round-trip to a draft", () => {
  const record = v6Listing({ currency: "EUR", amount: 900, shipping: 0 });
  const hash = verifyListingRecord(record)!.hash;
  const listings = parseOwnListings({ listings: [{ hash, record }, { hash: "00", record: "00" }] }, STORE);
  assert.equal(listings.length, 1);
  const draft = draftOf(listings[0]);
  assert.equal(draft.currency, "EUR");
  assert.equal(draft.price, 900);
  assert.equal(draft.contentRating, "sensitive");
  assert.deepEqual(parseOwnListings({ listings: [{ hash, record }] }, "ff".repeat(32)), []);
});

test("typed prices become minor units and back", () => {
  assert.equal(toMinorUnits("12.5", "USD"), 1250);
  assert.equal(toMinorUnits("12.50", "GBP"), 1250);
  assert.equal(toMinorUnits("1,200", "JPY"), 1200);
  assert.equal(toMinorUnits("25000", "SAT"), 25000);
  for (const bad of ["", "-1", "1.234", "abc", "1e3"]) assert.equal(toMinorUnits(bad, "USD"), null, bad);
  assert.equal(toMinorUnits("1.5", "JPY"), null);
  assert.equal(fromMinorUnits(1250, "USD"), "12.50");
  assert.equal(fromMinorUnits(1200, "JPY"), "1200");
  assert.equal(formatPrice(1250, "USD"), "$12.50");
  assert.equal(formatPrice(25000, "SAT"), "25,000 sats");
});

test("a store display name is required and at most 64 bytes", () => {
  assert.equal(displayNameError("  "), "Give the store a name.");
  assert.equal(displayNameError("Brook and Pine"), null);
  assert.equal(displayNameError("N".repeat(64)), null);
  assert.match(displayNameError("N".repeat(65)) ?? "", /64/);
  assert.equal(displayNameError("字".repeat(21)), null);
  assert.match(displayNameError("字".repeat(22)) ?? "", /64/);
});

test("a storefront image PUT names one field and refuses a hash that is not SHA-256 hex", () => {
  assert.deepEqual(storeImageChange("avatar", IMAGE), { avatar: IMAGE });
  assert.deepEqual(storeImageChange("banner", null), { banner: null });
  assert.throws(() => storeImageChange("avatar", "not a hash"), /did not keep the image/);
});

test("the profile reads the node's values and drops unknown ones", () => {
  const profile = parseProfile({ role: "seller", storeKey: STORE, name: "Brook and Pine", avatar: IMAGE, banner: "not a hash", shipsTo: ["US", 4], xmrAvailable: false, published: true });
  assert.equal(profile.role, "seller");
  assert.equal(profile.avatar, IMAGE);
  assert.equal(profile.banner, null);
  assert.equal(profile.acceptsLightning, false);
  assert.equal(profile.away, false);
  const paused = parseProfile({ role: "seller", storeKey: STORE, away: true, vacationNote: "in Paris until June" });
  assert.equal(paused.away, true);
  assert.equal("vacationNote" in paused, false);
  assert.equal(parseProfile({ role: "seller", storeKey: STORE, acceptsLightning: true }).acceptsLightning, true);
  assert.deepEqual(profile.shipsTo, ["US"]);
  assert.equal(parseProfile({ role: "admin", storeKey: STORE }).role, null);
  assert.throws(() => parseProfile({ role: "seller" }));
});

test("a sale parses with its fulfillment, and paid sales are the ones to ship", () => {
  const sale = parseSale({ id: "ab12", status: "Paid", totalSats: 28000, lines: [{ title: "Mug", kind: "physical", quantity: 1, priceSats: 25000 }], txids: ["c".repeat(64), "bad"], fulfillment: null });
  assert.equal(sale.lines[0].title, "Mug");
  assert.deepEqual(sale.txids, ["c".repeat(64)]);
  const shipped = parseSale({ id: "cd34", status: "Shipped", fulfillment: { carrier: "USPS", tracking: "9400" } });
  assert.equal(shipped.fulfillment?.carrier, "USPS");
  assert.equal(salesToShip([sale, shipped]), 1);
  assert.throws(() => parseSale({ id: "../x" }));
  assert.equal(sale.zcash, null);
  const zec = parseSale({ id: "ef56", status: "Awaiting payment", totalSats: 28000, address: null, payment: { rail: "zec", zcashAddress: "utest1example", amountZatoshi: "2277608", receivedZatoshi: "1000000", expired: true } });
  assert.deepEqual([zec.address, zec.zcash], ["", { address: "utest1example", amountZec: "0.02277608", receivedZec: "0.01", expired: true }], "a Zcash sale has no bitcoin address and counts in ZEC");
});

test("the test network seed is the vendor onion and a clearnet sale url is not", () => {
  const vendor = "http://gwyf5y2jzymphjmxatxjtivt5tow6oxtosjl53i2ylxr3wj365zourad.onion";
  assert.deepEqual(
    testNetworkSeeds({ seeds: [{ url: vendor }, { url: "http://example.com" }, { url: "https://shop.example" }] }),
    [vendor],
  );
});

test("follows parse, and a remote buyer order without payment terms yet reads", () => {
  assert.deepEqual(parseFollows({ follows: [{ url: "http://127.0.0.1:9", storeId: null, name: null, listings: 0, fetched: null, error: "offline" }] }), [
    { url: "http://127.0.0.1:9", storeId: null, name: null, listings: 0, fetched: null, error: "offline" },
  ]);
  const order = parseBuyerOrder({
    id: "o1",
    created: 1,
    seller: "Brook and Pine",
    storeId: STORE,
    totalSats: 28000,
    settlement: "direct",
    status: "Awaiting seller",
    remote: { delivered: true, fulfillment: { carrier: "USPS", tracking: "9400", note: "", content: "" } },
  });
  assert.equal(order.payment, null);
  assert.equal(order.remote?.delivered, true);
  assert.equal(order.remote?.fulfillment?.tracking, "9400");
});

/// Runs `run` against a node whose `POST /media` answers `respond`; `sent` collects the bodies it was sent.
async function withMediaNode(respond: () => Response, run: (sent: Blob[]) => Promise<void>): Promise<void> {
  const realFetch = globalThis.fetch;
  const sent: Blob[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    if (new URL(String(input)).pathname === "/pair") return Response.json({ token: "ab".repeat(32) });
    sent.push(init?.body as Blob);
    return respond();
  }) as typeof fetch;
  const forget = showPairingWith(async (trade) => {
    await trade("GOOD-CODE");
    return true;
  });
  try {
    await run(sent);
  } finally {
    forget();
    globalThis.fetch = realFetch;
  }
}

// The node keeps images up to 2 MiB (MAX_MEDIA in crates/node/src/media.rs)
// and answers a larger `POST /media` 413 with a plain-text body. These tests
// run without a canvas, as the native app does, so nothing is shrunk.
test("with no canvas an image over the node's cap is refused before it is sent, and the node's 413 reads the same", async () => {
  const tooLarge = { message: "That image is larger than 2 MB. Choose a smaller one." };
  await withMediaNode(
    () => new Response("request body too large", { status: 413 }),
    async (sent) => {
      await assert.rejects(uploadImage(new Blob([new Uint8Array(2 * 1024 * 1024 + 1)])), tooLarge);
      assert.equal(sent.length, 0);
      await assert.rejects(uploadImage(new Blob([new Uint8Array(2 * 1024 * 1024)])), tooLarge);
      assert.equal(sent.length, 1);
    },
  );
});

test("in a browser an image over the cap is sent shrunk, one that fits is sent as it is, and one that cannot be shrunk says so", async () => {
  const globals = globalThis as { document?: unknown; createImageBitmap?: unknown };
  const canvas = {
    width: 0,
    height: 0,
    getContext: () => ({ drawImage() {}, getImageData: () => ({ data: [] }) }),
    toBlob(done: (blob: Blob) => void, type: string) {
      done(new Blob([new Uint8Array(1000)], { type }));
    },
  };
  let decodes = true;
  // The page is also asked whether a home node served it: it was not.
  globals.document = { createElement: () => canvas, querySelector: () => null };
  globals.createImageBitmap = async () => {
    if (!decodes) throw new Error("not an image");
    return { width: 4000, height: 3000, close() {} };
  };
  try {
    await withMediaNode(
      () => Response.json({ hash: IMAGE }),
      async (sent) => {
        const small = new Blob([new Uint8Array(2 * 1024 * 1024)], { type: "image/png" });
        assert.equal(await uploadImage(small), IMAGE);
        assert.equal(sent[0], small);
        assert.equal(await uploadImage(new Blob([new Uint8Array(2 * 1024 * 1024 + 1)], { type: "image/png" })), IMAGE);
        assert.deepEqual([sent[1].size, sent[1].type, canvas.width, canvas.height], [1000, "image/jpeg", 2000, 1500]);
        decodes = false;
        await assert.rejects(uploadImage(new Blob([new Uint8Array(2 * 1024 * 1024 + 1)])), {
          message: "That image is larger than 2 MB and could not be shrunk to fit. Choose a smaller one.",
        });
        assert.equal(sent.length, 2);
      },
    );
  } finally {
    delete globals.document;
    delete globals.createImageBitmap;
  }
});
