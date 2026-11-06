// Monero direct pay: typed access to the node's /xmr routes.
//
// Amounts are piconero (1 XMR = 10^12) as bigint. The node sends them as
// decimal strings so no amount ever passes through a float.

import { paired } from './catalog.ts'

export type XmrNetwork = 'stagenet' | 'mainnet'
/** The rail of one checkout package. Escrow is bitcoin-only. */
export type Rail = 'btc' | 'xmr'

export const PICONERO_PER_XMR = 1_000_000_000_000n
export const XMR_CONFIRMATIONS = 10

export type XmrBalance = { unlocked: bigint; pending: bigint }
export type XmrWallet = {
  network: XmrNetwork
  /** `false` until the node runs with `--xmr-daemon`. */
  enabled: boolean
  /** `unlock-to-create`: the view keys are made at the next unlock. */
  keys: 'ready' | 'unlock-to-create'
  primaryAddress: string | null
  balance: XmrBalance | null
  scannedHeight: number | null
  tip: number | null
  lastSync: number | null
  syncError: string | null
}
export type XmrSubaddress = { network: XmrNetwork; index: number; address: string }
export type XmrStores = {
  network: XmrNetwork
  enabled: boolean
  stores: { seller: string; acceptsXmr: boolean }[]
}
/** What one Monero spend is allowed to do, exactly as the node enforces it. */
export type XmrTerms = {
  orderId: string
  network: XmrNetwork
  amount: bigint
  feeCap: bigint
  destination: string
  expiresInSecs: number
}
export type XmrQuote = XmrTerms & { quote: string }
export type XmrTicket = XmrTerms & { ticket: string }
/** The `payment` of an order on the Monero rail. */
export type XmrPayment = {
  network: XmrNetwork
  subaddress: string
  subaddressIndex: number
  amount: bigint
  received: bigint
  remaining: bigint
  overpaid: bigint
  /** `monero:` URI for an external wallet or a QR code. */
  uri: string
  satsPerXmr: number
  priceSats: number
  confirmationsNeeded: number
}
export type XmrBackend = {
  backend: 'none' | 'own-node' | 'remote-node'
  network: XmrNetwork
  privacy: 'none' | 'own-node' | 'remote-tor' | 'public'
  /** Plain words: who learns what from this backend. */
  reveals: string
  nodes: string[]
  proxied: boolean
  tip: number | null
  scannedHeight: number | null
  error: string | null
}
export type XmrRate = { satsPerXmr: number; currencies: Record<string, number> }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable Monero response.')
  return value as Record<string, unknown>
}
function count(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('The node returned an invalid Monero value.')
  return value
}
function countOrNull(value: unknown): number | null {
  return value == null ? null : count(value)
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value)
    throw new Error('The node returned an incomplete Monero response.')
  return value
}
function textOrNull(value: unknown): string | null {
  return typeof value === 'string' && value ? value : null
}
export function parsePiconero(value: unknown): bigint {
  if (typeof value !== 'string' || !/^\d{1,20}$/.test(value))
    throw new Error('The node returned an invalid Monero amount.')
  return BigInt(value)
}
function parseNetwork(value: unknown): XmrNetwork {
  if (value !== 'stagenet' && value !== 'mainnet')
    throw new Error('The node named an unknown Monero network.')
  return value
}
function oneOf<T extends string>(value: unknown, allowed: readonly T[]): T {
  if (typeof value !== 'string' || !allowed.includes(value as T))
    throw new Error('The node returned an unknown Monero state.')
  return value as T
}

/** `1.5`, `0.000000000001`: exact, trailing zeros dropped. */
export function formatXmr(piconero: bigint): string {
  const whole = piconero / PICONERO_PER_XMR
  const fraction = (piconero % PICONERO_PER_XMR).toString().padStart(12, '0').replace(/0+$/, '')
  return fraction ? `${whole}.${fraction}` : `${whole}`
}

export function parseXmrWallet(value: unknown): XmrWallet {
  const r = record(value)
  const balance = r.balance == null ? null : record(r.balance)
  return {
    network: parseNetwork(r.network),
    enabled: r.enabled === true,
    keys: oneOf(r.keys, ['ready', 'unlock-to-create'] as const),
    primaryAddress: textOrNull(r.primaryAddress),
    balance: balance && {
      unlocked: parsePiconero(balance.unlockedPiconero),
      pending: parsePiconero(balance.pendingPiconero)
    },
    scannedHeight: countOrNull(r.scannedHeight),
    tip: countOrNull(r.tip),
    lastSync: countOrNull(r.lastSync),
    syncError: textOrNull(r.syncError)
  }
}
export function parseXmrStores(value: unknown): XmrStores {
  const r = record(value)
  const stores = Array.isArray(r.stores) ? r.stores.map(record) : []
  return {
    network: parseNetwork(r.network),
    enabled: r.enabled === true,
    stores: stores.map(store => ({ seller: text(store.seller), acceptsXmr: store.acceptsXmr === true }))
  }
}
export function parseXmrTerms(value: unknown): XmrTerms {
  const r = record(value)
  return {
    orderId: text(r.orderId),
    network: parseNetwork(r.network),
    amount: parsePiconero(r.amountPiconero),
    feeCap: parsePiconero(r.feeCapPiconero),
    destination: text(r.destination),
    expiresInSecs: count(r.expiresInSecs)
  }
}
/** The Monero payment of an order, or `null` for an order on the bitcoin rail. */
export function parseXmrPayment(value: unknown): XmrPayment | null {
  if (!value || typeof value !== 'object' || (value as Record<string, unknown>).rail !== 'xmr') return null
  const r = record(value)
  const amount = parsePiconero(r.amountPiconero)
  const received = parsePiconero(r.receivedPiconero ?? '0')
  return {
    network: parseNetwork(r.network),
    subaddress: text(r.subaddress),
    subaddressIndex: count(r.subaddressIndex),
    amount,
    received,
    remaining: r.remainingPiconero == null ? (amount > received ? amount - received : 0n) : parsePiconero(r.remainingPiconero),
    overpaid: r.overpaidPiconero == null ? 0n : parsePiconero(r.overpaidPiconero),
    uri: text(r.uri),
    satsPerXmr: count(r.satsPerXmr),
    priceSats: count(r.priceSats),
    confirmationsNeeded: count(r.confirmationsNeeded)
  }
}
/** `xmrBackend` from `GET /network`. */
export function parseXmrBackend(value: unknown): XmrBackend {
  const r = record(value)
  return {
    backend: oneOf(r.backend, ['none', 'own-node', 'remote-node'] as const),
    network: parseNetwork(r.network),
    privacy: oneOf(r.privacy, ['none', 'own-node', 'remote-tor', 'public'] as const),
    reveals: text(r.reveals),
    nodes: Array.isArray(r.nodes) ? r.nodes.map(text) : [],
    proxied: r.proxied === true,
    tip: countOrNull(r.tip),
    scannedHeight: countOrNull(r.scannedHeight),
    error: textOrNull(r.error)
  }
}
/** `xmr` from `GET /rates`: `null` with `--no-rates` or no answer. Monero cannot be priced then. */
export function parseXmrRate(value: unknown): XmrRate | null {
  if (value == null) return null
  const r = record(value)
  const currencies: Record<string, number> = {}
  for (const [code, price] of Object.entries(record(r.currencies ?? {})))
    if (typeof price === 'number' && Number.isFinite(price) && price > 0) currencies[code] = price
  return { satsPerXmr: count(r.satsPerXmr), currencies }
}

/** The rail a package is sent with. Monero only for direct pay to a store that accepts it. */
export function railFor(
  settlement: 'direct' | 'escrow',
  wanted: Rail,
  acceptsXmr: boolean
): Rail {
  if (wanted === 'btc') return 'btc'
  if (settlement === 'escrow') throw new Error('Escrow is Bitcoin-only. Pay in Monero with direct pay.')
  if (!acceptsXmr) throw new Error('This store does not accept Monero.')
  return 'xmr'
}

function send(path: string, body?: object, method = 'POST') {
  return paired(path, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {})
  })
}
const orderPath = (id: string, rest: string) => `/xmr/orders/${encodeURIComponent(id)}${rest}`

export async function fetchXmrWallet(): Promise<XmrWallet> {
  return parseXmrWallet(await paired('/xmr/wallet'))
}
export async function newXmrSubaddress(): Promise<XmrSubaddress> {
  const r = record(await send('/xmr/wallet/subaddress'))
  return { network: parseNetwork(r.network), index: count(r.index), address: text(r.address) }
}
export async function fetchXmrStores(): Promise<XmrStores> {
  return parseXmrStores(await paired('/xmr/stores'))
}
export async function setStoreAcceptsXmr(seller: string, accept: boolean): Promise<XmrStores> {
  return parseXmrStores(await send('/xmr/stores', { seller, accept }, 'PUT'))
}
export async function quoteXmrPayment(id: string): Promise<XmrQuote> {
  const r = record(await send(orderPath(id, '/pay/quote')))
  return { ...parseXmrTerms(r), quote: text(r.quote) }
}
export async function issueXmrTicket(quote: string, passphrase: string): Promise<XmrTicket> {
  const r = record(await send('/xmr/spend-tickets', { quote, passphrase }))
  return { ...parseXmrTerms(r), ticket: text(r.ticket) }
}
/** Queue the payment (202). The order then shows `paying`, and later `payTxid` or `payError`. `confirm` is required on mainnet. */
export async function payXmrOrder(id: string, ticket: string, confirm = false): Promise<void> {
  await send(orderPath(id, '/pay'), confirm ? { ticket, confirm } : { ticket })
}
export async function fetchXmrPayment(id: string): Promise<XmrPayment | null> {
  return parseXmrPayment(record(await paired(`/orders/${encodeURIComponent(id)}`)).payment)
}
export async function fetchXmrBackend(): Promise<XmrBackend> {
  return parseXmrBackend(record(await paired('/network')).xmrBackend)
}
export async function fetchXmrRate(): Promise<XmrRate | null> {
  return parseXmrRate(record(await paired('/rates')).xmr)
}
