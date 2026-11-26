// Zcash direct pay: typed access to the node's /zec routes.
//
// Amounts are zatoshi (1 ZEC = 10^8) as bigint. The node sends them as
// decimal strings so no amount ever passes through a float. An order's
// address carries one Orchard-protocol receiver, so the payment lands in
// Ironwood, the pool that pays orders. Escrow is bitcoin-only.

import { paired } from './catalog.ts'

export type ZecNetwork = 'testnet' | 'mainnet'

export const ZATOSHI_PER_ZEC = 100_000_000n
/** About 12.5 minutes at Zcash's 25-second blocks. */
export const ZEC_CONFIRMATIONS = 30

export type ZecStores = {
  network: ZecNetwork
  /** `false` until the node runs with `--zec-indexer`. */
  enabled: boolean
  /** `own` is this node's own store: its record goes to other nodes. The rest are demo stores. `problem` is why the own store, with Zcash on, would decline a Zcash order now. */
  stores: { seller: string; own: boolean; acceptsZec: boolean; problem: string | null }[]
}
export type ZecBackend = {
  enabled: boolean
  backend: 'none' | 'own-node' | 'remote-indexer' | 'mock'
  network: ZecNetwork
  privacy: 'none' | 'own-node' | 'remote-tor' | 'public'
  /** Plain words: who learns what from the indexer. */
  reveals: string
  servers: string[]
  active: string | null
  /** What the indexer says it runs. */
  software: string | null
  proxied: boolean
  tip: number | null
  scannedHeight: number | null
  error: string | null
}
export type ZecRate = { satsPerZec: number; currencies: Record<string, number> }
export type ZecBalance = { confirmed: bigint; pending: bigint; orchard: bigint }
export type ZecWallet = {
  network: ZecNetwork
  enabled: boolean
  keys: 'ready' | 'unlock-to-create'
  /** Index 0. Order addresses start at 1. */
  receiveAddress: string | null
  balance: ZecBalance | null
  scannedHeight: number | null
  tip: number | null
  lastSync: number | null
  syncError: string | null
}
export type ZecTerms = {
  orderId: string
  network: ZecNetwork
  amount: bigint
  feeCap: bigint
  destination: string
  expiresInSecs: number
}
export type ZecQuote = ZecTerms & { quote: string }
export type ZecTicket = ZecTerms & { ticket: string }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable Zcash response.')
  return value as Record<string, unknown>
}
function count(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('The node returned an invalid Zcash value.')
  return value
}
function countOrNull(value: unknown): number | null {
  return value == null ? null : count(value)
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value)
    throw new Error('The node returned an incomplete Zcash response.')
  return value
}
function textOrNull(value: unknown): string | null {
  return typeof value === 'string' && value ? value : null
}
function oneOf<T extends string>(value: unknown, allowed: readonly T[]): T {
  if (typeof value !== 'string' || !allowed.includes(value as T))
    throw new Error('The node returned an unknown Zcash state.')
  return value as T
}
function parseNetwork(value: unknown): ZecNetwork {
  return oneOf(value, ['testnet', 'mainnet'] as const)
}

export function parseZatoshi(value: unknown): bigint {
  if (typeof value !== 'string' || !/^\d{1,20}$/.test(value))
    throw new Error('The node returned an invalid Zcash amount.')
  return BigInt(value)
}

/** `1.5`, `0.00000001`: exact, trailing zeros dropped. */
export function formatZec(zatoshi: bigint): string {
  const whole = zatoshi / ZATOSHI_PER_ZEC
  const fraction = (zatoshi % ZATOSHI_PER_ZEC).toString().padStart(8, '0').replace(/0+$/, '')
  return fraction ? `${whole}.${fraction}` : `${whole}`
}

export function parseZecStores(value: unknown): ZecStores {
  const r = record(value)
  const stores = Array.isArray(r.stores) ? r.stores.map(record) : []
  return {
    network: parseNetwork(r.network),
    enabled: r.enabled === true,
    stores: stores.map(store => ({ seller: text(store.seller), own: store.own === true, acceptsZec: store.acceptsZec === true, problem: textOrNull(store.problem) }))
  }
}

/** `zecBackend` from `GET /network`. */
export function parseZecBackend(value: unknown): ZecBackend {
  const r = record(value)
  return {
    enabled: r.enabled === true,
    backend: oneOf(r.backend, ['none', 'own-node', 'remote-indexer', 'mock'] as const),
    network: parseNetwork(r.network),
    privacy: oneOf(r.privacy, ['none', 'own-node', 'remote-tor', 'public'] as const),
    reveals: text(r.reveals),
    servers: Array.isArray(r.servers) ? r.servers.map(text) : [],
    active: textOrNull(r.active),
    software: textOrNull(r.software),
    proxied: r.proxied === true,
    tip: countOrNull(r.tip),
    scannedHeight: countOrNull(r.scannedHeight),
    error: textOrNull(r.error)
  }
}

/** `zec` from `GET /rates`: `null` with `--no-rates` or no answer. Zcash cannot be priced then. */
export function parseZecRate(value: unknown): ZecRate | null {
  if (value == null) return null
  const r = record(value)
  const currencies: Record<string, number> = {}
  for (const [code, price] of Object.entries(record(r.currencies ?? {})))
    if (typeof price === 'number' && Number.isFinite(price) && price > 0) currencies[code] = price
  return { satsPerZec: count(r.satsPerZec), currencies }
}

export async function fetchZecStores(): Promise<ZecStores> {
  return parseZecStores(await paired('/zec/stores'))
}
export async function setStoreAcceptsZec(seller: string, accept: boolean): Promise<ZecStores> {
  return parseZecStores(
    await paired('/zec/stores', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ seller, accept })
    })
  )
}
export async function fetchZecBackend(): Promise<ZecBackend> {
  return parseZecBackend(record(await paired('/network')).zecBackend)
}
export async function enableZcash(indexers: string): Promise<ZecBackend> {
  return parseZecBackend(
    await paired('/zec/enable', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ indexers })
    })
  )
}

export function parseZecWallet(value: unknown): ZecWallet {
  const r = record(value)
  const balance = r.balance == null ? null : record(r.balance)
  return {
    network: parseNetwork(r.network),
    enabled: r.enabled === true,
    keys: oneOf(r.keys, ['ready', 'unlock-to-create'] as const),
    receiveAddress: textOrNull(r.receiveAddress),
    balance: balance && {
      confirmed: parseZatoshi(balance.confirmedZatoshi),
      pending: parseZatoshi(balance.pendingZatoshi),
      orchard: parseZatoshi(balance.orchardZatoshi)
    },
    scannedHeight: countOrNull(r.scannedHeight),
    tip: countOrNull(r.tip),
    lastSync: countOrNull(r.lastSync),
    syncError: textOrNull(r.syncError)
  }
}

function parseZecTerms(value: Record<string, unknown>): ZecTerms {
  return {
    orderId: text(value.orderId),
    network: parseNetwork(value.network),
    amount: parseZatoshi(value.amountZatoshi),
    feeCap: parseZatoshi(value.feeCapZatoshi),
    destination: text(value.destination),
    expiresInSecs: count(value.expiresInSecs)
  }
}

function send(path: string, body?: object) {
  return paired(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {})
  })
}
const orderPath = (id: string, rest: string) => `/zec/orders/${encodeURIComponent(id)}${rest}`

export async function fetchZecWallet(): Promise<ZecWallet> {
  return parseZecWallet(await paired('/zec/wallet'))
}
export async function quoteZecPayment(id: string): Promise<ZecQuote> {
  const r = record(await send(orderPath(id, '/pay/quote')))
  return { ...parseZecTerms(r), quote: text(r.quote) }
}
export async function issueZecTicket(quote: string, passphrase: string): Promise<ZecTicket> {
  const r = record(await send('/zec/spend-tickets', { quote, passphrase }))
  return { ...parseZecTerms(r), ticket: text(r.ticket) }
}
/** Queue the payment (202). `confirm` is required on mainnet. */
export async function payZecOrder(id: string, ticket: string, confirm = false): Promise<void> {
  await send(orderPath(id, '/pay'), confirm ? { ticket, confirm } : { ticket })
}
