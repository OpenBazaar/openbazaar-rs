import type { NetworkName } from './catalog'

export type ChainPrivacy = {
  backend: string
  privacy: string
  reveals: string
  fallback: string | null
  fallbackActive: boolean
}

export type NetworkInfo = {
  network: NetworkName
  confirmations: number
  publicChainServer: boolean
  lightning: string
  silentPayments: string
  clearnet: boolean
  privacyWarning: string | null
  chain: ChainPrivacy | null
  /** This node's onion, once the listener has published it. */
  onion: string | null
  /** Compact-filter height, when the chain backend has one. */
  scannedHeight: number | null
  syncing: boolean
  /** The chain backend reported a failure and is not syncing. */
  chainFailed: boolean
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
  if (value === 'testnet4' || value === 'mainnet') return value
  throw new Error('The node returned an unknown bitcoin network. Payments are disabled.')
}

function chainPrivacy(value: unknown): ChainPrivacy | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const r = value as Record<string, unknown>
  if (typeof r.backend !== 'string' || typeof r.privacy !== 'string' || typeof r.reveals !== 'string') return null
  return {
    backend: r.backend,
    privacy: r.privacy,
    reveals: r.reveals,
    fallback: typeof r.fallback === 'string' ? r.fallback : null,
    fallbackActive: r.fallbackActive === true
  }
}

function chainField(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  return value as Record<string, unknown>
}

function height(value: unknown): number | null {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) return null
  return value
}

function onionAddress(value: unknown): string | null {
  if (typeof value !== 'string' || !/^[a-z2-7]{56}\.onion$/.test(value)) return null
  return value
}

/** Chain-height phrase for the status banner. */
export function chainHeightLabel(info: NetworkInfo): string {
  if (info.syncing) {
    return info.scannedHeight == null ? 'Chain syncing' : `Chain syncing · height ${info.scannedHeight}`
  }
  if (info.chainFailed) return 'Chain sync failed'
  if (info.scannedHeight == null) return 'Chain height unavailable'
  return `Chain height ${info.scannedHeight}`
}

/** Network and chain height for the status banner. The onion stays off this line. */
export function networkStatus(info: NetworkInfo): string {
  const network = info.network === 'testnet4' ? 'Testnet4 · test coins, no value' : 'Mainnet'
  return `${network} · ${chainHeightLabel(info)}`
}

/** The node's own account of its transport and chain backend. */
export function networkInfo(value: unknown): NetworkInfo {
  const r = record(value)
  return {
    network: network(r.network),
    confirmations: number(r.confirmations),
    publicChainServer: r.publicChainServer === true,
    lightning: String(r.lightning ?? 'unavailable'),
    silentPayments: String(r.silentPayments ?? 'unavailable'),
    clearnet: r.clearnet === true,
    privacyWarning: typeof r.privacyWarning === 'string' && r.privacyWarning ? r.privacyWarning : null,
    chain: chainPrivacy(r.chainBackend),
    onion: onionAddress(r.onion ?? r.letterOnion),
    scannedHeight: height(chainField(r.chainBackend)?.scannedHeight),
    syncing: chainField(r.chainBackend)?.syncing === true,
    chainFailed: !((chainField(r.chainBackend)?.syncing) === true) && typeof chainField(r.chainBackend)?.error === 'string' && chainField(r.chainBackend)?.error !== ''
  }
}
