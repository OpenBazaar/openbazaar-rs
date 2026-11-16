import assert from "node:assert/strict";
import { test } from "node:test";

import { fiatView, formatFiat, parseWalletRates, rateAgeLabel, STALE_AFTER_SECS, TEST_COINS_NOTE, UNUSABLE_AFTER_SECS, type CoinRate } from "./walletFiat.ts";

const NOW = 1_800_000_000_000;
const fresh: CoinRate = { currencies: { USD: 100_000, EUR: 92_000, GBP: 78_000, JPY: 15_000_000 }, asOf: NOW, fixed: false };
const aged = (secs: number): CoinRate => ({ ...fresh, asOf: NOW - secs * 1000 });

test("formats each display currency with its own decimals", () => {
  assert.equal(formatFiat(3.4167, "USD"), "$3.42");
  assert.equal(formatFiat(3.4167, "EUR"), "€3.42");
  assert.equal(formatFiat(1234.5, "GBP"), "£1,234.50");
  assert.equal(formatFiat(512.6, "JPY"), "¥513");
  assert.equal(formatFiat(0, "USD"), "$0.00");
  assert.equal(formatFiat(0, "JPY"), "¥0");
});

test("a value below the smallest unit says so instead of rounding to zero", () => {
  // 1 sat at $100k is $0.001; at ¥15M it is ¥0.15.
  assert.equal(formatFiat(0.001, "USD"), "< $0.01");
  assert.equal(formatFiat(0.15, "JPY"), "< ¥1");
  assert.equal(formatFiat(0.01, "USD"), "$0.01");
});

test("converts sats and XMR amounts at the chosen currency's price", () => {
  const view = (coins: number, currency: "USD" | "JPY") => fiatView({ coins, rate: fresh, currency, testCoins: false, now: NOW });
  assert.deepEqual(view(3_420 / 1e8, "USD"), { estimate: "≈ $3.42", note: null });
  assert.deepEqual(view(3_420 / 1e8, "JPY"), { estimate: "≈ ¥513", note: null });
  assert.deepEqual(view(1 / 1e8, "USD"), { estimate: "< $0.01", note: null });
  assert.deepEqual(view(1 / 1e8, "JPY"), { estimate: "< ¥1", note: null });
});

test("test coins are priced at mainnet and labelled as worthless", () => {
  assert.deepEqual(fiatView({ coins: 0.0001, rate: fresh, currency: "USD", testCoins: true, now: NOW }), {
    estimate: "≈ $10.00",
    note: "at mainnet price · test coins, no value",
  });
  assert.equal(fiatView({ coins: 0.0001, rate: { ...fresh, fixed: true }, currency: "USD", testCoins: true, now: NOW }).note, "at a fixed test rate · test coins, no value");
  assert.deepEqual(fiatView({ coins: 0.0001, rate: null, currency: "USD", testCoins: true, now: NOW }), { estimate: null, note: TEST_COINS_NOTE });
});

test("a missing price shows no number", () => {
  const none = { estimate: null, note: "Price unavailable" };
  assert.deepEqual(fiatView({ coins: 1, rate: null, currency: "USD", testCoins: false, now: NOW }), none);
  assert.deepEqual(fiatView({ coins: 1, rate: undefined, currency: "USD", testCoins: false, now: NOW }), none);
  assert.deepEqual(fiatView({ coins: 1, rate: { ...fresh, currencies: { USD: 100_000 } }, currency: "EUR", testCoins: false, now: NOW }), none, "fixed rates are USD only");
});

test("an old price says how old it is, and a very old one is not shown", () => {
  assert.equal(fiatView({ coins: 1, rate: aged(STALE_AFTER_SECS - 1), currency: "USD", testCoins: false, now: NOW }).note, null);
  assert.deepEqual(fiatView({ coins: 1, rate: aged(2 * 3600), currency: "USD", testCoins: false, now: NOW }), { estimate: "≈ $100,000.00", note: "price from 2h ago" });
  assert.equal(fiatView({ coins: 1, rate: aged(2 * 3600), currency: "USD", testCoins: true, now: NOW }).note, "at mainnet price · test coins, no value · price from 2h ago");
  assert.deepEqual(fiatView({ coins: 1, rate: aged(UNUSABLE_AFTER_SECS + 1), currency: "USD", testCoins: false, now: NOW }), { estimate: null, note: "Price unavailable" });
  assert.equal(rateAgeLabel(20 * 60), "price from 20 min ago");
});

test("the age keeps growing while the node does not answer again", () => {
  const rates = parseWalletRates({ currencies: { USD: 100_000 }, ageSecs: 60 }, NOW);
  assert.equal(fiatView({ coins: 1, rate: rates.btc, currency: "USD", testCoins: false, now: NOW }).note, null);
  assert.equal(fiatView({ coins: 1, rate: rates.btc, currency: "USD", testCoins: false, now: NOW + 3 * 3600_000 }).note, "price from 3h ago");
});

test("reads BTC and XMR prices from /rates and drops bad ones", () => {
  const rates = parseWalletRates({
    base: "BTC", ageSecs: 30, currencies: { USD: 100_000, EUR: 0, CAD: 130_000, JPY: "x" },
    xmr: { base: "XMR", satsPerXmr: 300_000, source: "kraken", ageSecs: 90, currencies: { USD: 300 } },
    zec: { base: "ZEC", currencies: { USD: 50 } },
  }, NOW);
  assert.deepEqual(rates.btc, { currencies: { USD: 100_000 }, asOf: NOW - 30_000, fixed: false });
  assert.deepEqual(rates.xmr, { currencies: { USD: 300 }, asOf: NOW - 90_000, fixed: false });
  assert.deepEqual(parseWalletRates({ currencies: { USD: 50_000 }, source: "fixed" }, NOW).btc, { currencies: { USD: 50_000 }, asOf: null, fixed: true });
  assert.deepEqual(parseWalletRates({ base: "BTC", currencies: {}, xmr: null }, NOW), { btc: null, xmr: null }, "--no-rates or Tor down");
  assert.deepEqual(parseWalletRates(null, NOW), { btc: null, xmr: null });
});
