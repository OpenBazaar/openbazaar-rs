// Paying a pasted Lightning invoice and withdrawing the Lightning node's
// on-chain funds: typed access to /ln/send* and /ln/withdraw*.

import { paired } from './catalog.ts'
import type { LnNetwork } from './lightning.ts'
import { bitcoinAddressKind, bitcoinAddressOf } from './walletSend.ts'

export type LnInvoiceQuote = {
  quote: string
  network: LnNetwork
  paymentHash: string
  amountSats: number
  feeCapSats: number
  payee: string
  description: string
  expiresInSecs: number
}
export type LnPaid = { paymentHash: string; amountSats: number; status: 'paid' | 'pending' | 'failed'; feeSats: number | null }
export type LnWithdrawQuote = { quote: string; network: LnNetwork; address: string; amountSats: number; max: boolean; expiresInSecs: number }

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
function network(value: unknown): LnNetwork {
  if (value === 'signet' || value === 'testnet4' || value === 'regtest' || value === 'bitcoin') return value
  throw new Error('The node named an unknown Lightning network.')
}

const INVOICE_NETWORKS: [string, LnNetwork][] = [['lnbcrt', 'regtest'], ['lntbs', 'signet'], ['lntb', 'testnet4'], ['lnbc', 'bitcoin']]
const NAMES: Record<LnNetwork, string> = { signet: 'Mutinynet (signet)', testnet4: 'testnet4', regtest: 'regtest', bitcoin: 'mainnet' }

export function lnInvoiceOf(input: string): string {
  return input.trim().replace(/^lightning:/i, '')
}

/** Why `invoice` cannot be paid on `net`, before asking the node. The node decodes it fully. */
export function lnInvoiceProblem(invoice: string, net: LnNetwork): string | null {
  const value = lnInvoiceOf(invoice).toLowerCase()
  if (!value) return 'Paste the Lightning invoice to pay.'
  const on = INVOICE_NETWORKS.find(([prefix]) => value.startsWith(prefix))?.[1]
  if (!on || !/^[a-z0-9]+$/.test(value)) return 'That is not a Lightning invoice. It starts with "ln".'
  if (on !== net) return `That invoice is for ${NAMES[on]}. This Lightning node is on ${NAMES[net]}.`
  return null
}

/** Why the Lightning node cannot withdraw to `address` on `net`. */
export function lnAddressProblem(address: string, net: LnNetwork): string | null {
  const value = bitcoinAddressOf(address)
  if (!value) return 'Enter the address to send to.'
  const kind = bitcoinAddressKind(value)
  if (!kind) return 'That is not a bitcoin address.'
  const expected = net === 'bitcoin' ? 'mainnet' : net === 'regtest' ? 'regtest' : 'testnet4'
  if (kind !== expected) return `That address is not on ${NAMES[net]}, the Lightning node's network.`
  return null
}

function post(path: string, body: object) {
  return paired(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
}

export function parseLnInvoiceQuote(value: unknown): LnInvoiceQuote {
  const r = record(value)
  return {
    quote: text(r.quote),
    network: network(r.network),
    paymentHash: text(r.paymentHash),
    amountSats: count(r.amountSats),
    feeCapSats: count(r.feeCapSats),
    payee: text(r.payee),
    description: typeof r.description === 'string' ? r.description : '',
    expiresInSecs: count(r.expiresInSecs)
  }
}

export function parseLnWithdrawQuote(value: unknown): LnWithdrawQuote {
  const r = record(value)
  return { quote: text(r.quote), network: network(r.network), address: text(r.address), amountSats: count(r.amountSats), max: r.max === true, expiresInSecs: count(r.expiresInSecs) }
}

export async function quoteLnInvoice(invoice: string): Promise<LnInvoiceQuote> {
  return parseLnInvoiceQuote(await post('/ln/send/quote', { invoice: lnInvoiceOf(invoice) }))
}

export async function payLnInvoice(quote: string, passphrase: string): Promise<LnPaid> {
  const r = record(await post('/ln/send', { quote, passphrase, confirm: true }))
  const status = r.status === 'paid' || r.status === 'failed' ? r.status : 'pending'
  return { paymentHash: text(r.paymentHash), amountSats: count(r.amountSats), status, feeSats: typeof r.feeSats === 'number' ? r.feeSats : null }
}

export async function quoteLnWithdraw(address: string, amountSats?: number): Promise<LnWithdrawQuote> {
  const body = { address: bitcoinAddressOf(address), ...(amountSats === undefined ? { max: true } : { amountSats }) }
  return parseLnWithdrawQuote(await post('/ln/withdraw/quote', body))
}

export async function withdrawLn(quote: string, passphrase: string): Promise<{ txid: string; amountSats: number; max: boolean }> {
  const r = record(await post('/ln/withdraw', { quote, passphrase, confirm: true }))
  return { txid: text(r.txid), amountSats: count(r.amountSats), max: r.max === true }
}
