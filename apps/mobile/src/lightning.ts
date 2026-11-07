// Lightning direct pay: typed access to the node's /ln routes.
//
// The seller's node issues one BOLT 11 invoice per order. Amounts are whole
// sats here; the node keeps millisatoshis and rounds up for display.

import { paired } from './catalog.ts'

/** Mutinynet is a signet; test mode runs Lightning there. */
export type LnNetwork = 'signet' | 'testnet4' | 'regtest' | 'bitcoin'

export type LnChannel = {
  id: string
  peer: string
  capacitySats: number
  outboundSats: number
  inboundSats: number
  ready: boolean
  usable: boolean
}
export type LnBalances = { lightningSats: number; inboundSats: number; onchainSats: number }
export type LnStatus = {
  /** `false` until the node runs with `--lightning`. */
  enabled: boolean
  /** Why the node refuses Lightning (mainnet, until Lightning runs over Tor), or `null`. */
  blocked: string | null
  network: LnNetwork | null
  running: boolean
  /** `unlock-to-create`: the Lightning keys are made at the next unlock. */
  keys: 'ready' | 'unlock-to-create' | null
  lsp: { nodeId: string; address: string; connected: boolean } | null
  lspNotice: string | null
  lspAcknowledged: boolean
  /** Plain sentences: who learns what. */
  reveals: string[]
  nodeId: string | null
  balances: LnBalances | null
  channels: LnChannel[]
  error: string | null
}
export type LnStores = {
  enabled: boolean
  blocked: string | null
  lspAcknowledged: boolean
  /** Shown once, before the first store takes Lightning through an LSP. */
  lspNotice: string | null
  stores: { seller: string; own: boolean; acceptsLightning: boolean }[]
}
/** The `payment` of an order on the Lightning rail. */
export type LnInvoice = {
  invoice: string
  /** `lightning:` URI for a wallet or a QR code. */
  uri: string
  paymentHash: string
  amountSats: number
  expiresAt: number
  /** The invoice routes through the seller's LSP, which opens a channel when it is paid. */
  jit: boolean
  expired: boolean
  /** This node issued the invoice (it is the seller): pay it from another wallet. */
  local: boolean
  lspFeeSats: number | null
}
/** What one Lightning payment is allowed to do, exactly as the node enforces it. */
export type LnTerms = {
  orderId: string
  network: LnNetwork
  paymentHash: string
  amountSats: number
  feeCapSats: number
  expiresInSecs: number
}
export type LnQuote = LnTerms & { quote: string }
export type LnTicket = LnTerms & { ticket: string }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable Lightning response.')
  return value as Record<string, unknown>
}
function count(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('The node returned an invalid Lightning value.')
  return value
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value)
    throw new Error('The node returned an incomplete Lightning response.')
  return value
}
function textOrNull(value: unknown): string | null {
  return typeof value === 'string' && value ? value : null
}
const NETWORKS: readonly LnNetwork[] = ['signet', 'testnet4', 'regtest', 'bitcoin']
function parseNetwork(value: unknown): LnNetwork {
  if (typeof value !== 'string' || !NETWORKS.includes(value as LnNetwork))
    throw new Error('The node named an unknown Lightning network.')
  return value as LnNetwork
}
const msatToSats = (value: unknown) => Math.ceil(count(value) / 1000)

/**
 * Why the node refuses Lightning, from the `lightning` of `/catalog` or the
 * body of `/ln/status`; `null` when it may run. A node that names mainnet
 * Lightning is refused too: mainnet Lightning would show its IP address.
 */
export function lnBlockedReason(value: unknown): string | null {
  if (!value || typeof value !== 'object') return null
  const r = value as Record<string, unknown>
  if (typeof r.blocked === 'string' && r.blocked) return r.blocked
  return r.network === 'bitcoin' ? 'Lightning on mainnet is disabled until its connections run over Tor.' : null
}

/** The stores buyers are offered Lightning for: none while the node refuses it. */
export function lnStoreKeys(stores: { key: string; acceptsLightning: boolean }[], lightning: unknown): Set<string> {
  if (lnBlockedReason(lightning)) return new Set()
  return new Set(stores.filter(store => store.acceptsLightning).map(store => store.key))
}

/** How the UI names a Lightning network. */
export function lnNetworkLabel(network: LnNetwork): string {
  return network === 'signet'
    ? 'Mutinynet signet · test coins, no value'
    : network === 'testnet4'
      ? 'Testnet4 · test coins, no value'
      : network === 'regtest'
        ? 'Regtest · local test chain'
        : 'Mainnet'
}

export function parseLnStatus(value: unknown): LnStatus {
  const r = record(value)
  const blocked = lnBlockedReason(r)
  if (r.enabled !== true || blocked)
    return { enabled: false, blocked, network: null, running: false, keys: null, lsp: null, lspNotice: null, lspAcknowledged: false, reveals: [], nodeId: null, balances: null, channels: [], error: null }
  const lsp = r.lsp == null ? null : record(r.lsp)
  const balances = r.balances == null ? null : record(r.balances)
  return {
    enabled: true,
    blocked: null,
    network: parseNetwork(r.network),
    running: r.running === true,
    keys: r.keys === 'ready' ? 'ready' : 'unlock-to-create',
    lsp: lsp && { nodeId: text(lsp.nodeId), address: text(lsp.address), connected: lsp.connected === true },
    lspNotice: textOrNull(r.lspNotice),
    lspAcknowledged: r.lspAcknowledged === true,
    reveals: Array.isArray(r.reveals) ? r.reveals.map(text) : [],
    nodeId: textOrNull(r.nodeId),
    balances: balances && {
      lightningSats: count(balances.lightningSats),
      inboundSats: count(balances.inboundSats),
      onchainSats: count(balances.onchainSats)
    },
    channels: (Array.isArray(r.channels) ? r.channels : []).map(item => {
      const c = record(item)
      return {
        id: text(c.id),
        peer: text(c.peer),
        capacitySats: count(c.capacitySats),
        outboundSats: count(c.outboundSats),
        inboundSats: count(c.inboundSats),
        ready: c.ready === true,
        usable: c.usable === true
      }
    }),
    error: textOrNull(r.error)
  }
}
export function parseLnStores(value: unknown): LnStores {
  const r = record(value)
  const blocked = lnBlockedReason(r)
  return {
    enabled: r.enabled === true && !blocked,
    blocked,
    lspAcknowledged: r.lspAcknowledged === true,
    lspNotice: textOrNull(r.lspNotice),
    stores: (Array.isArray(r.stores) ? r.stores : []).map(item => {
      const s = record(item)
      return { seller: text(s.seller), own: s.own === true, acceptsLightning: s.acceptsLightning === true }
    })
  }
}
/** The Lightning invoice of an order, or `null` for an order on another rail. */
export function parseLnInvoice(value: unknown): LnInvoice | null {
  if (!value || typeof value !== 'object' || (value as Record<string, unknown>).rail !== 'ln') return null
  const r = record(value)
  const invoice = text(r.invoice)
  if (!/^ln[a-z0-9]+$/.test(invoice)) throw new Error('The node returned an unreadable Lightning invoice.')
  return {
    invoice,
    uri: text(r.uri),
    paymentHash: text(r.paymentHash),
    amountSats: msatToSats(r.amountMsat),
    expiresAt: count(r.expiresAt),
    jit: r.jit === true,
    expired: r.expired === true,
    local: r.issuer === 'local',
    lspFeeSats: r.lspFeeMsat == null ? null : msatToSats(r.lspFeeMsat)
  }
}
export function parseLnTerms(value: unknown): LnTerms {
  const r = record(value)
  return {
    orderId: text(r.orderId),
    network: parseNetwork(r.network),
    paymentHash: text(r.paymentHash),
    amountSats: count(r.amountSats),
    feeCapSats: count(r.feeCapSats),
    expiresInSecs: count(r.expiresInSecs)
  }
}

function send(path: string, body?: object, method = 'POST') {
  return paired(path, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {})
  })
}
const orderPath = (id: string, rest: string) => `/ln/orders/${encodeURIComponent(id)}${rest}`

export async function fetchLnStatus(): Promise<LnStatus> {
  return parseLnStatus(await paired('/ln/status'))
}
export async function fetchLnStores(): Promise<LnStores> {
  return parseLnStores(await paired('/ln/stores'))
}
/** The first opt-in through an LSP needs `acknowledgeLsp` once the user has read `lspNotice`. */
export async function setStoreAcceptsLightning(seller: string, accept: boolean, acknowledgeLsp = false): Promise<LnStores> {
  return parseLnStores(await send('/ln/stores', { seller, accept, acknowledgeLsp }, 'PUT'))
}
export async function quoteLnPayment(id: string): Promise<LnQuote> {
  const r = record(await send(orderPath(id, '/pay/quote')))
  return { ...parseLnTerms(r), quote: text(r.quote) }
}
export async function issueLnTicket(quote: string, passphrase: string): Promise<LnTicket> {
  const r = record(await send('/ln/spend-tickets', { quote, passphrase }))
  return { ...parseLnTerms(r), ticket: text(r.ticket) }
}
/** Start the payment (202). `confirm` is required on mainnet. */
export async function payLnOrder(id: string, ticket: string, confirm = false): Promise<void> {
  await send(orderPath(id, '/pay'), confirm ? { ticket, confirm } : { ticket })
}
export async function openLnChannel(peer: string, sats: number, passphrase: string): Promise<string> {
  return text(record(await send('/ln/channels', { peer, sats, passphrase })).channelId)
}
export async function closeLnChannel(id: string, passphrase: string, force = false): Promise<void> {
  await send(`/ln/channels/${encodeURIComponent(id)}/close`, force ? { passphrase, force } : { passphrase })
}
