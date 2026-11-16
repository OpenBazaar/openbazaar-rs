// Fiat estimates beside wallet balances, from the node's `GET /rates` (fetched
// over Tor by the node). An estimate is shown only with a usable price: a
// missing or very old one shows "Price unavailable", never a guessed number.

import { paired, type CurrencyCode, type RateTable } from './catalog.ts'

const CURRENCIES: readonly CurrencyCode[] = ['USD', 'EUR', 'GBP', 'JPY']

/**
 * One coin's fiat prices. `asOf` is when the node's price source answered, in
 * ms; it is computed on arrival so the age keeps growing if the node stops
 * answering. Fixed test rates have no age.
 */
export type CoinRate = { currencies: RateTable; asOf: number | null; fixed: boolean }
/** Zcash is priced too, but the wallet has no Zcash balance to show it beside. */
export type WalletRates = { btc: CoinRate | null; xmr: CoinRate | null }

/** The node refreshes prices every 5 minutes; older than this means the source is failing. */
export const STALE_AFTER_SECS = 15 * 60
/** A price this old is not shown at all. */
export const UNUSABLE_AFTER_SECS = 24 * 60 * 60

function coinRate(value: unknown, now: number): CoinRate | null {
  if (!value || typeof value !== 'object') return null
  const r = value as { currencies?: unknown; ageSecs?: unknown; source?: unknown }
  const prices = (r.currencies && typeof r.currencies === 'object' ? r.currencies : {}) as Record<string, unknown>
  const currencies: RateTable = {}
  for (const code of CURRENCIES) {
    const price = prices[code]
    if (typeof price === 'number' && Number.isFinite(price) && price > 0) currencies[code] = price
  }
  if (!Object.keys(currencies).length) return null
  const age = typeof r.ageSecs === 'number' && Number.isFinite(r.ageSecs) && r.ageSecs >= 0 ? r.ageSecs : null
  return { currencies, asOf: age == null ? null : now - age * 1000, fixed: r.source === 'fixed' }
}

/** BTC prices at the top level of `/rates`, XMR under `xmr`. */
export function parseWalletRates(value: unknown, now = Date.now()): WalletRates {
  const r = (value && typeof value === 'object' ? value : {}) as { xmr?: unknown }
  return { btc: coinRate(r, now), xmr: coinRate(r.xmr, now) }
}

export async function fetchWalletRates(): Promise<WalletRates> {
  return parseWalletRates(await paired('/rates'))
}

/** "$3.42", "¥512", or "< $0.01" for a value too small to show in the currency's smallest unit. */
export function formatFiat(value: number, currency: CurrencyCode): string {
  const format = new Intl.NumberFormat('en-US', { style: 'currency', currency })
  const smallest = 10 ** -(format.resolvedOptions().maximumFractionDigits ?? 2)
  return value > 0 && value < smallest ? `< ${format.format(smallest)}` : format.format(value)
}

export function rateAgeLabel(ageSecs: number): string {
  const minutes = Math.floor(ageSecs / 60)
  return minutes < 60 ? `price from ${minutes} min ago` : `price from ${Math.floor(minutes / 60)}h ago`
}

export type FiatView = { estimate: string | null; note: string | null }

export const TEST_COINS_NOTE = 'Test coins · no monetary value'

/**
 * The line under a balance. `coins` is the amount in whole BTC or XMR.
 * Test coins are priced at the mainnet rate and say so, since they have no value.
 */
export function fiatView({ coins, rate, currency, testCoins, now = Date.now() }: { coins: number; rate: CoinRate | null | undefined; currency: CurrencyCode; testCoins: boolean; now?: number }): FiatView {
  const price = rate?.currencies[currency]
  const ageSecs = rate?.asOf == null ? null : Math.max(0, (now - rate.asOf) / 1000)
  if (!rate || !price || (ageSecs != null && ageSecs > UNUSABLE_AFTER_SECS)) return { estimate: null, note: testCoins ? TEST_COINS_NOTE : 'Price unavailable' }
  const notes: string[] = []
  if (testCoins) notes.push(rate.fixed ? 'at a fixed test rate · test coins, no value' : 'at mainnet price · test coins, no value')
  if (ageSecs != null && ageSecs > STALE_AFTER_SECS) notes.push(rateAgeLabel(ageSecs))
  const value = formatFiat(coins * price, currency)
  return { estimate: value.startsWith('<') ? value : `≈ ${value}`, note: notes.length ? notes.join(' · ') : null }
}
