import {
  openJson,
  paired,
  parseBuyerOrder,
  parseNetworkName,
  type BuyerOrder,
  type NetworkName
} from './catalog'
import { parseXmrBackend, type XmrBackend } from './xmr'
import { parseZecBackend, type ZecBackend } from './zec'

export type ChainBackend = {
  backend: string
  privacy: string
  reveals: string
  peers: number | null
  scannedHeight: number | null
  syncing: boolean
  error: string | null
  fallback: string | null
  fallbackActive: boolean
}
export type NetworkInfo = {
  xmrBackend?: XmrBackend | null
  zecBackend?: ZecBackend | null
  network: NetworkName
  confirmations: number
  publicChainServer: boolean
  lightning: string
  silentPayments: string
  proxy: string | null
  clearnet: boolean
  privacyWarning: string | null
  chainBackend: ChainBackend | null
  /** This node's onion, once the listener has published it. */
  onion: string | null
  /** A SOCKS greeting to the configured proxy succeeded. Null when no proxy is set. */
  proxyReachable: boolean | null
}
export type WalletActivity = {
  txid: string
  direction: 'in' | 'out'
  sats: number
  confirmed: boolean
  height: number | null
  time: number | null
  label: string
}
/** Copying stays on this device. Opening an explorer does not use the node's Tor settings. */
export const EXTERNAL_TX_NOTE =
  "Copying a transaction ID stays on this device. A browser does not inherit this node's Tor or proxy settings, so a public explorer would see the transaction ID and the browser's address."
export type NodeWallet = {
  network: NetworkName
  balance: {
    totalSats: number
    confirmedSats: number
    pendingSats: number
    spendableSats: number
  }
  receiveAddress: string
  lastSync: number | null
  syncError: string | null
  activity: WalletActivity[]
}
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable wallet response.')
  return value as Record<string, unknown>
}
function number(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('The node returned an invalid amount.')
  return value
}
function network(value: unknown): NetworkName {
  const name = parseNetworkName(value)
  if (!name)
    throw new Error(
      'The node returned an unknown bitcoin network. Payments are disabled.'
    )
  return name
}
function textOrNull(value: unknown): string | null {
  return typeof value === 'string' && value ? value : null
}
function chainBackend(value: unknown): ChainBackend | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const backend = value as Record<string, unknown>
  return {
    backend: typeof backend.backend === 'string' ? backend.backend : '',
    privacy: typeof backend.privacy === 'string' ? backend.privacy : '',
    reveals: typeof backend.reveals === 'string' ? backend.reveals : '',
    peers: typeof backend.peers === 'number' ? backend.peers : null,
    scannedHeight:
      typeof backend.scannedHeight === 'number' ? backend.scannedHeight : null,
    syncing: backend.syncing === true,
    error: textOrNull(backend.error),
    fallback: textOrNull(backend.fallback),
    fallbackActive: backend.fallbackActive === true
  }
}
export { connectionRows, privacyStatus, type ConnectionRow } from './walletConnection'
export async function fetchNetwork(): Promise<NetworkInfo> {
  const r = record(await openJson('/network'))
  return {
    xmrBackend: r.xmrBackend ? parseXmrBackend(r.xmrBackend) : null,
    zecBackend: r.zecBackend ? parseZecBackend(r.zecBackend) : null,
    network: network(r.network),
    confirmations: number(r.confirmations),
    publicChainServer: r.publicChainServer === true,
    lightning: String(r.lightning ?? 'unavailable'),
    silentPayments: String(r.silentPayments ?? 'unavailable'),
    proxy: textOrNull(r.proxy),
    clearnet: r.clearnet === true,
    privacyWarning: textOrNull(r.privacyWarning),
    chainBackend: chainBackend(r.chainBackend),
    onion: textOrNull(r.onion) ?? textOrNull(r.letterOnion),
    proxyReachable: r.proxyReachable === true ? true : r.proxyReachable === false ? false : null
  }
}
export async function fetchWallet(): Promise<NodeWallet> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 12000)
  try {
    const r = record(await paired('/wallet', { signal: controller.signal })),
      balance = record(r.balance)
    if (typeof r.receiveAddress !== 'string' || !r.receiveAddress)
      throw new Error('The node did not return a receive address.')
    return {
      network: network(r.network),
      balance: {
        totalSats: number(balance.totalSats),
        confirmedSats: number(balance.confirmedSats),
        pendingSats: number(balance.pendingSats),
        spendableSats: number(balance.spendableSats)
      },
      receiveAddress: r.receiveAddress,
      lastSync: r.lastSync == null ? null : number(r.lastSync),
      syncError: typeof r.syncError === 'string' ? r.syncError : null,
      activity: activityList(r.activity)
    }
  } catch (error) {
    if (controller.signal.aborted) throw new Error('The wallet request timed out. Check your node and refresh the wallet.')
    throw error
  } finally {
    clearTimeout(timer)
  }
}

function activityList(value: unknown): WalletActivity[] {
  if (!Array.isArray(value)) return []
  return value.map(item => {
    const row = record(item)
    if (row.direction !== 'in' && row.direction !== 'out') {
      throw new Error('The node returned an unreadable wallet activity.')
    }
    if (typeof row.txid !== 'string' || !row.txid || typeof row.label !== 'string' || !row.label) {
      throw new Error('The node returned an unreadable wallet activity.')
    }
    return {
      txid: row.txid,
      direction: row.direction,
      sats: number(row.sats),
      confirmed: row.confirmed === true,
      height: typeof row.height === 'number' ? row.height : null,
      time: typeof row.time === 'number' ? row.time : null,
      label: row.label
    }
  })
}
export async function newWalletAddress(): Promise<{
  network: NetworkName
  address: string
}> {
  const r = record(
    await paired('/wallet/address', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}'
    })
  )
  if (typeof r.address !== 'string' || !r.address)
    throw new Error('The node did not return a new address.')
  return { network: network(r.network), address: r.address }
}
/** Pay with a spend ticket the user confirmed for this order. */
export async function payOrder(
  id: string,
  activeNetwork: NetworkName,
  ticket: string
): Promise<BuyerOrder> {
  return parseBuyerOrder(
    await paired(`/orders/${encodeURIComponent(id)}/pay`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(
        activeNetwork === 'mainnet' ? { ticket, confirm: true } : { ticket }
      )
    })
  )
}
/** Replace an unconfirmed payment with one at `feeRateSatVb`, paying at most `maxFeeSats` in fees. */
export async function bumpPayment(
  id: string,
  activeNetwork: NetworkName,
  bump: { feeRateSatVb: number; maxFeeSats: number; passphrase: string }
): Promise<{ txid: string; replacedTxid: string; feeSats: number }> {
  const r = record(
    await paired(`/orders/${encodeURIComponent(id)}/pay/bump`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(
        activeNetwork === 'mainnet' ? { ...bump, confirm: true } : bump
      )
    })
  )
  if (typeof r.txid !== 'string' || typeof r.replacedTxid !== 'string')
    throw new Error('The node did not return the new transaction.')
  return { txid: r.txid, replacedTxid: r.replacedTxid, feeSats: number(r.feeSats) }
}
export function amountDue(totalSats: number, receivedSats: number): number {
  return Math.max(0, totalSats - receivedSats)
}
export function bitcoinAmount(sats: number): string {
  return (sats / 100_000_000).toFixed(8)
}
export function bitcoinUri(address: string, sats?: number): string {
  return `bitcoin:${address}${sats === undefined ? '' : `?amount=${bitcoinAmount(sats)}`}`
}
