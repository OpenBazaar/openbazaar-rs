import { waitingForTor, type TorStatus } from './torStatus.ts'

export type ConnectionChain = {
  backend: string
  privacy: string
  peers: number | null
  scannedHeight: number | null
  syncing: boolean
  error: string | null
  fallbackActive: boolean
}

export type ConnectionInfo = {
  proxy: string | null
  proxyReachable: boolean | null
  chainBackend: ConnectionChain | null
  tor?: TorStatus | null
}

export type ConnectionRow = { label: string; value: string }

/** Banner warning only. The connection card says what the proxy did and what to do next. */
export function privacyStatus(info: ConnectionInfo & { privacyWarning?: string | null }): string {
  const backend = info.chainBackend
  const tor = info.tor
  if (tor && waitingForTor(tor)) {
    if (tor.state === 'connecting') return `Connecting to Tor…${tor.progress != null ? ` ${tor.progress}%` : ''}`
    if (tor.state === 'degraded') return `Tor having trouble${tor.error ? `: ${tor.error}` : ''}`
    return `Tor not connected: ${tor.error ?? 'the node keeps trying'}`
  }
  if (info.proxyReachable === false) return 'Tor is not running'
  if (backend?.fallbackActive) return 'Public fallback active'
  if (backend?.error) return 'Chain transport failed'
  if (info.privacyWarning && !info.proxy) return 'Private route unavailable'
  return ''
}

function proxyHost(proxy: string): string {
  const host = proxy.replace(/^socks5h:\/\//, '').replace(/^socks5:\/\//, '')
  return /^[A-Za-z0-9.:[\]-]+$/.test(host) ? host : 'the configured proxy'
}

/** Labelled rows for the wallet's connection card. No flags, no repeated status line. */
export function connectionRows(info: ConnectionInfo): ConnectionRow[] {
  return [
    { label: 'Chain data source', value: chainSource(info) },
    { label: 'Proxy', value: proxyLine(info) },
    { label: 'What the server can see', value: serverSees(info) }
  ]
}

function chainSource(info: ConnectionInfo): string {
  const backend = info.chainBackend
  if (!backend) return 'Not reported.'
  if (backend.fallbackActive) {
    return 'Public chain server, while no filter peer is answering. It supplies the tip, fees, and broadcasts.'
  }
  if (backend.error && !backend.syncing) return 'The chain source is not answering.'
  if (backend.backend === 'compact-filters') {
    if (backend.syncing) return 'Compact block filters, still syncing.'
    const peers = backend.peers ?? 0
    if (peers > 0) {
      return `Compact block filters from ${peers} ${peers === 1 ? 'peer' : 'peers'}.`
    }
    if (backend.scannedHeight != null) return 'Compact block filters.'
    return 'Compact block filters. Waiting for a peer.'
  }
  if (backend.backend === 'own-node') return 'Your own Bitcoin node.'
  if (backend.backend === 'esplora') {
    return backend.privacy === 'own-server' ? 'Your own chain server.' : 'A public chain server.'
  }
  if (backend.backend === 'offline') return 'Offline.'
  return 'Not reported.'
}

function proxyLine(info: ConnectionInfo): string {
  const tor = info.tor
  if (tor?.mode === 'builtin') {
    if (tor.state === 'connected') return 'Built-in Tor, connected.'
    if (tor.state === 'degraded') {
      return `Built-in Tor is having trouble${tor.error ? `: ${tor.error}` : ''}. The node keeps trying.`
    }
    if (tor.state === 'failed') {
      return `Built-in Tor could not connect${tor.error ? `: ${tor.error}` : ''}. Check this computer's internet connection; the node keeps trying.`
    }
    return `Built-in Tor is connecting to the Tor network${tor.progress != null ? ` (${tor.progress}%)` : ''}. Payments and prices wait until it is ready.`
  }
  if (!info.proxy) {
    return 'None. Public servers are not contacted from this computer. To send those requests through Tor, point the node at a local proxy such as 127.0.0.1:9050 and refresh.'
  }
  const host = proxyHost(info.proxy)
  const carried = info.chainBackend?.scannedHeight != null && !info.chainBackend.error
  if (info.proxyReachable === true) return `${host} answered.`
  if (carried) return `Chain data is arriving through ${host}.`
  if (info.proxyReachable === false) {
    return `${host} did not answer. Start Tor on that address, then refresh the wallet.`
  }
  return `Waiting on ${host}. Start Tor if this does not change, then refresh the wallet.`
}

function serverSees(info: ConnectionInfo): string {
  const backend = info.chainBackend
  if (!backend) return 'Not reported.'
  if (backend.privacy === 'own-server') {
    return 'Only your own chain server sees the addresses this wallet follows.'
  }
  if (backend.privacy === 'own-node' || backend.backend === 'own-node') {
    return 'Only your own Bitcoin node sees the addresses this wallet follows.'
  }
  if (backend.fallbackActive || backend.privacy === 'public') {
    return 'A public chain server sees the tip, fee estimates, and any transaction sent to it. Your addresses stay on this node.'
  }
  if (info.proxy) {
    return 'Filter peers see a Tor exit and which blocks this node downloads. Your addresses stay on this node.'
  }
  return 'Public peers receive nothing from this computer. Your addresses stay on this node.'
}
