// Sending Monero from the node wallet: typed access to /xmr/wallet/send*.
// Amounts are piconero as bigint, as in xmr.ts.

import { paired } from './catalog.ts'
import { PICONERO_PER_XMR, parsePiconero, type XmrNetwork } from './xmr.ts'

export type XmrSendQuote = {
  quote: string
  network: XmrNetwork
  address: string
  amount: bigint
  fee: bigint
  feeCap: bigint
  total: bigint
  expiresInSecs: number
}
export type XmrSent = { txid: string; amount: bigint; fee: bigint }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable send response.')
  return value as Record<string, unknown>
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value) throw new Error('The node returned an incomplete send quote.')
  return value
}

/** XMR typed by the user (up to 12 decimals) in piconero, or `null`. */
export function parseXmrAmount(input: string): bigint | null {
  const value = input.replace(/[\s,_]/g, '')
  const match = /^(\d*)(?:\.(\d{0,12}))?$/.exec(value)
  if (!match || (!match[1] && !match[2])) return null
  const piconero = BigInt(match[1] || '0') * PICONERO_PER_XMR + BigInt((match[2] ?? '').padEnd(12, '0') || '0')
  return piconero > 0n && piconero < 2n ** 64n ? piconero : null
}

const BASE58 = /^[1-9A-HJ-NP-Za-km-z]+$/
const PREFIXES: Record<XmrNetwork, string> = { mainnet: '48', stagenet: '57' }

/**
 * Why `address` cannot be sent to on `network`, before asking the node.
 * The node decodes and checks the address fully.
 */
export function xmrAddressProblem(address: string, network: XmrNetwork): string | null {
  const value = address.trim().replace(/^monero:/i, '').split('?')[0]
  if (!value) return 'Enter the address to send to.'
  if (!BASE58.test(value) || (value.length !== 95 && value.length !== 106)) return 'That is not a Monero address.'
  if (value.length === 106) return 'Integrated addresses (with a payment ID) are not supported. Ask for a standard address or subaddress.'
  const on = (Object.keys(PREFIXES) as XmrNetwork[]).find(net => PREFIXES[net].includes(value[0]))
  if (!on) return 'That is not a Monero address.'
  if (on !== network)
    return network === 'stagenet'
      ? `That is a ${on} address. This wallet holds stagenet coins; send only to a stagenet address.`
      : `That is a ${on} address. This wallet holds real Monero; send only to a mainnet address.`
  return null
}

function post(path: string, body: object) {
  return paired(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
}

export function parseXmrSendQuote(value: unknown): XmrSendQuote {
  const r = record(value)
  if (r.network !== 'stagenet' && r.network !== 'mainnet') throw new Error('The node quoted an unknown Monero network.')
  return {
    quote: text(r.quote),
    network: r.network,
    address: text(r.address),
    amount: parsePiconero(r.amountPiconero),
    fee: parsePiconero(r.feePiconero),
    feeCap: parsePiconero(r.feeCapPiconero),
    total: parsePiconero(r.totalPiconero),
    expiresInSecs: typeof r.expiresInSecs === 'number' ? r.expiresInSecs : 60
  }
}

export async function quoteXmrSend(address: string, amount: bigint): Promise<XmrSendQuote> {
  const to = address.trim().replace(/^monero:/i, '').split('?')[0]
  return parseXmrSendQuote(await post('/xmr/wallet/send/quote', { address: to, amountPiconero: amount.toString() }))
}

export async function sendXmr(quote: string, passphrase: string): Promise<XmrSent> {
  const r = record(await post('/xmr/wallet/send', { quote, passphrase, confirm: true }))
  return { txid: text(r.txid), amount: parsePiconero(r.amountPiconero), fee: parsePiconero(r.feePiconero) }
}
