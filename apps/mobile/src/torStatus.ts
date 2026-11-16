/** `tor` from `GET /network`: whether the node's Tor works, and what waits on it. */
export type TorStatus = {
  /** `builtin`: the node's own Tor. `system`: a Tor proxy already on this computer. */
  mode: 'builtin' | 'system' | 'none'
  /** `connected` only once Tor can build circuits, not when its port merely listens. */
  state: 'connecting' | 'connected' | 'failed' | 'off'
  progress: number | null
  error: string | null
  onionServices: { name: string; published: boolean }[]
}

export type TorTone = 'ok' | 'busy' | 'down'
export type TorFeature = { name: string; state: string }

const text = (value: unknown): string | null => (typeof value === 'string' && value ? value : null)
const record = (value: unknown): Record<string, unknown> =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {}

export function parseTorStatus(value: unknown): TorStatus | null {
  if (!value || typeof value !== 'object') return null
  const r = record(value)
  const mode = r.mode === 'builtin' || r.mode === 'system' ? r.mode : 'none'
  const state = r.state === 'connecting' || r.state === 'connected' || r.state === 'failed' ? r.state : 'off'
  const progress = typeof r.progress === 'number' && r.progress >= 0 && r.progress <= 100 ? Math.round(r.progress) : null
  const onionServices = Array.isArray(r.onionServices)
    ? r.onionServices.map(onion => ({ name: text(record(onion).name) ?? 'unknown', published: record(onion).published === true }))
    : []
  return { mode, state, progress, error: text(r.error), onionServices }
}

/** Tor is in use but cannot carry anything yet. */
export function waitingForTor(tor: TorStatus | null | undefined): boolean {
  return !!tor && tor.mode !== 'none' && tor.state !== 'connected'
}

/** The sidebar line: a dot tone, a short label, and a longer title. `null` without Tor. */
export function torIndicator(tor: TorStatus | null | undefined): { tone: TorTone; label: string; title: string } | null {
  if (!tor || tor.mode === 'none') return null
  const kind = tor.mode === 'builtin' ? 'built-in' : 'system'
  if (tor.state === 'connected') return { tone: 'ok', label: `Tor connected (${kind})`, title: 'Tor is connected and can build circuits.' }
  if (tor.state === 'connecting') {
    const progress = tor.progress != null ? ` ${tor.progress}%` : ''
    return { tone: 'busy', label: `Tor connecting…${progress}`, title: tor.error ?? `Connecting to the Tor network (${kind}).` }
  }
  return { tone: 'down', label: `Tor not connected (${kind})`, title: tor.error ?? 'Tor is not connected.' }
}

/** One plain sentence about the connection, for the top of the details. */
export function torSummary(tor: TorStatus): string {
  const which = tor.mode === 'builtin' ? 'OpenBazaar’s built-in Tor' : 'The Tor proxy on this computer'
  if (tor.mode === 'none') return 'This node does not use Tor. Public servers are not contacted.'
  if (tor.state === 'connected') return `${which} is connected.`
  if (tor.state === 'connecting') return `${which} is connecting to the Tor network${tor.progress != null ? ` (${tor.progress}%)` : ''}. Payments, prices, and peers wait until it is ready.`
  return `${which} is not connected${tor.error ? `: ${tor.error}` : ''}. The node keeps trying.`
}

const ONION_NAMES: Record<string, string> = {
  store: 'Public store page',
  'remote access': 'Remote access for your devices',
  lightning: 'Lightning payments'
}
export const onionLabel = (name: string) => ONION_NAMES[name] ?? name

/** What depends on Tor, from the rest of `GET /network`, in plain words. */
export function torFeatures(network: unknown, tor: TorStatus | null): TorFeature[] {
  const r = record(network)
  const waiting = waitingForTor(tor) ? 'Waiting for Tor' : null
  const features: TorFeature[] = []
  const chain = record(r.chainBackend)
  if (Object.keys(chain).length) {
    const peers = typeof chain.peers === 'number' ? chain.peers : 0
    const height = typeof chain.scannedHeight === 'number' ? chain.scannedHeight.toLocaleString() : null
    features.push({
      name: 'Bitcoin peers',
      state: waiting ?? (chain.fallbackActive === true ? 'No filter peer yet; a public server gives the tip and fees'
        : peers > 0 ? `${peers} ${peers === 1 ? 'peer' : 'peers'}${height ? `, scanned to block ${height}` : ''}`
        : chain.syncing === true ? `Syncing${height ? `, block ${height}` : ''}` : height ? `Scanned to block ${height}` : 'Looking for peers')
    })
  }
  const xmr = record(r.xmrBackend)
  if (xmr.backend && xmr.backend !== 'none' && xmr.proxied === true) {
    features.push({ name: 'Monero', state: waiting ?? (xmr.connecting === true ? 'Connecting to a Monero node' : text(xmr.error) ? 'Nodes are not answering' : 'Connected') })
  }
  const ln = record(r.lightning)
  if (ln.enabled === true) features.push({ name: 'Lightning', state: waiting ?? (ln.running === true ? 'Running' : 'Starting') })
  const zec = record(r.zecBackend)
  if (zec.enabled === true && zec.proxied === true) {
    features.push({ name: 'Zcash indexer', state: waiting ?? (text(zec.error) ? 'Not answering' : zec.active ? 'Connected' : 'Connecting') })
  }
  if (r.liveRates === true && r.proxy) features.push({ name: 'Exchange rates', state: waiting ?? 'Fetched over Tor' })
  if (r.proxy) features.push({ name: 'Package tracking', state: waiting ?? 'Looked up over Tor' })
  return features
}
