import { NodeApiError, paired, parseNetworkName, type NetworkName } from './catalog.ts'
import { passphraseError } from './walletLock.ts'

/** A send from the node's bitcoin wallet, exactly as the node will enforce it. */
export type BitcoinSendQuote = {
  quote: string
  network: NetworkName
  address: string
  amountSats: number
  feeSats: number
  feeCapSats: number
  feeRateSatVb: number
  totalSats: number
  max: boolean
  linkCoins: boolean
  heldBackSats: number
  expiresInSecs: number
}
export type BitcoinSent = { txid: string; amountSats: number; feeSats: number }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable send response.')
  return value as Record<string, unknown>
}
function count(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('The node returned an invalid amount.')
  return value
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value) throw new Error('The node returned an incomplete send quote.')
  return value
}

/** Whole sats from what the user typed; separators allowed. `null` when it is not a positive whole number. */
export function parseSats(input: string): number | null {
  const digits = input.trim().replace(/[\s,_]/g, '')
  if (!/^\d+$/.test(digits)) return null
  const sats = Number(digits)
  return Number.isSafeInteger(sats) && sats > 0 ? sats : null
}

/** The address in a pasted address or `bitcoin:` URI. */
export function bitcoinAddressOf(input: string): string {
  const trimmed = input.trim()
  const uri = /^bitcoin:([^?]*)/i.exec(trimmed)
  return uri ? uri[1] : trimmed
}

/**
 * Why `address` cannot be sent to on `network`, before asking the node.
 * The node checks the address fully; this catches the wrong network early.
 */
export function bitcoinAddressProblem(address: string, network: NetworkName): string | null {
  const value = bitcoinAddressOf(address)
  if (!value) return 'Enter the address to send to.'
  const lower = value.toLowerCase()
  const kind = /^bcrt1/.test(lower) ? 'regtest'
    : /^bc1/.test(lower) || /^[13]/.test(value) ? 'mainnet'
    : /^tb1/.test(lower) || /^[mn2]/.test(value) ? 'testnet4'
    : null
  if (!kind || !/^[a-zA-Z0-9]{26,90}$/.test(value)) return 'That is not a bitcoin address.'
  if (kind !== network)
    return network === 'testnet4'
      ? `That is a ${kind} address. This wallet holds testnet4 coins; send only to a testnet4 address.`
      : `That is a ${kind} address. This wallet holds real bitcoin; send only to a mainnet address.`
  return null
}

export function parseBitcoinQuote(value: unknown): BitcoinSendQuote {
  const r = record(value)
  const network = parseNetworkName(r.network)
  if (!network) throw new Error('The node quoted an unknown bitcoin network.')
  if (typeof r.feeRateSatVb !== 'number' || !(r.feeRateSatVb > 0)) throw new Error('The node quoted an invalid fee rate.')
  return {
    quote: text(r.quote),
    network,
    address: text(r.address),
    amountSats: count(r.amountSats),
    feeSats: count(r.feeSats),
    feeCapSats: count(r.feeCapSats),
    feeRateSatVb: r.feeRateSatVb,
    totalSats: count(r.totalSats),
    max: r.max === true,
    linkCoins: r.linkCoins === true,
    heldBackSats: count(r.heldBackSats ?? 0),
    expiresInSecs: count(r.expiresInSecs)
  }
}

function post(path: string, body: object) {
  return paired(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
}

export async function quoteBitcoinSend(input: { address: string; amountSats?: number; linkCoins?: boolean }): Promise<BitcoinSendQuote> {
  const body = {
    address: bitcoinAddressOf(input.address),
    ...(input.amountSats === undefined ? { max: true } : { amountSats: input.amountSats }),
    ...(input.linkCoins ? { linkCoins: true } : {})
  }
  return parseBitcoinQuote(await post('/wallet/send/quote', body))
}

export async function sendBitcoin(quote: string, passphrase: string): Promise<BitcoinSent> {
  const r = record(await post('/wallet/send', { quote, passphrase, confirm: true }))
  return { txid: text(r.txid), amountSats: count(r.amountSats), feeSats: count(r.feeSats) }
}

/** `true` when the node refused because the quote is used, expired, or out of date. */
export function quoteExpired(e: unknown): boolean {
  return e instanceof NodeApiError && e.status === 409
}

/** `true` when the node's refusal says it left out linked change, so a larger send needs the user to link it. */
export function heldBack(message: string): boolean {
  return message.includes('held back')
}

/** What to tell the user about a failed quote or send. */
export function sendError(e: unknown, fallback: string): string {
  if (e instanceof NodeApiError && e.status === 502)
    return `The node could not reach the network, so nothing was sent. Check its connection (and Tor) and try again. ${e.message}`
  return passphraseError(e, fallback)
}
