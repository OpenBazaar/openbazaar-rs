// Sending Zcash from the node wallet: typed access to /zec/wallet/send*.
// Amounts are zatoshi as bigint, as in zec.ts. The node builds the same
// Ironwood payment an order uses, and only on testnet.

import { paired } from './catalog.ts'
import { parseZatoshi, ZATOSHI_PER_ZEC, type ZecNetwork } from './zec.ts'

export type ZecSendQuote = {
  quote: string
  network: ZecNetwork
  address: string
  amount: bigint
  fee: bigint
  feeCap: bigint
  total: bigint
  expiresInSecs: number
}
export type ZecSent = { txid: string; amount: bigint; fee: bigint }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable send response.')
  return value as Record<string, unknown>
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value) throw new Error('The node returned an incomplete send quote.')
  return value
}

/** ZEC typed by the user (up to 8 decimals) in zatoshi, or `null`. */
export function parseZecAmount(input: string): bigint | null {
  const value = input.replace(/[\s,_]/g, '')
  const match = /^(\d*)(?:\.(\d{0,8}))?$/.exec(value)
  if (!match || (!match[1] && !match[2])) return null
  const zatoshi = BigInt(match[1] || '0') * ZATOSHI_PER_ZEC + BigInt((match[2] ?? '').padEnd(8, '0') || '0')
  return zatoshi > 0n && zatoshi < 2n ** 64n ? zatoshi : null
}

/**
 * Why `address` cannot be sent to before asking the node.
 * The node decodes the unified address and checks the receiver.
 */
export function zecAddressProblem(address: string, network: ZecNetwork): string | null {
  if (network !== 'testnet') return 'Sending Zcash from this wallet is testnet only.'
  const value = address.trim().replace(/^zcash:/i, '').split('?')[0]
  if (!value) return 'Enter the address to send to.'
  if (!value.startsWith('utest1') || value.length < 20 || /\s/.test(value)) return 'That is not a Zcash testnet address.'
  return null
}

function post(path: string, body: object) {
  return paired(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
}

export function parseZecSendQuote(value: unknown): ZecSendQuote {
  const r = record(value)
  if (r.network !== 'testnet') throw new Error('The node quoted a Zcash network this wallet does not send on.')
  return {
    quote: text(r.quote),
    network: 'testnet',
    address: text(r.address),
    amount: parseZatoshi(r.amountZatoshi),
    fee: parseZatoshi(r.feeZatoshi),
    feeCap: parseZatoshi(r.feeCapZatoshi),
    total: parseZatoshi(r.totalZatoshi),
    expiresInSecs: typeof r.expiresInSecs === 'number' ? r.expiresInSecs : 60
  }
}

export async function quoteZecSend(address: string, amount: bigint): Promise<ZecSendQuote> {
  const to = address.trim().replace(/^zcash:/i, '').split('?')[0]
  return parseZecSendQuote(await post('/zec/wallet/send/quote', { address: to, amountZatoshi: amount.toString() }))
}

export async function sendZec(quote: string, passphrase: string): Promise<ZecSent> {
  const r = record(await post('/zec/wallet/send', { quote, passphrase, confirm: true }))
  return { txid: text(r.txid), amount: parseZatoshi(r.amountZatoshi), fee: parseZatoshi(r.feeZatoshi) }
}
