import {
  paired,
  parseBuyerOrder,
  parseNetworkName,
  type BuyerOrder,
  type NetworkName
} from './catalog'

export type NetworkInfo = {
  network: NetworkName
  confirmations: number
  publicChainServer: boolean
  lightning: string
  silentPayments: string
}
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
export async function fetchNetwork(): Promise<NetworkInfo> {
  const r = record(await paired('/network'))
  return {
    network: network(r.network),
    confirmations: number(r.confirmations),
    publicChainServer: r.publicChainServer === true,
    lightning: String(r.lightning ?? 'unavailable'),
    silentPayments: String(r.silentPayments ?? 'unavailable')
  }
}
export async function fetchWallet(): Promise<NodeWallet> {
  const r = record(await paired('/wallet')),
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
    syncError: typeof r.syncError === 'string' ? r.syncError : null
  }
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
export async function payOrder(
  id: string,
  activeNetwork: NetworkName,
  confirmed = false
): Promise<BuyerOrder> {
  if (activeNetwork === 'mainnet' && !confirmed)
    throw new Error('Confirm the mainnet payment first.')
  return parseBuyerOrder(
    await paired(`/orders/${encodeURIComponent(id)}/pay`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(activeNetwork === 'mainnet' ? { confirm: true } : {})
    })
  )
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
export function txUrl(network: NetworkName, txid: string): string {
  return `https://mempool.space/${network === 'testnet4' ? 'testnet4/' : ''}tx/${txid}`
}
