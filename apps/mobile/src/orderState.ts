// Which payment controls an order may show.
//
// The node reports one status string for both rails
// (Awaiting payment -> Payment seen -> Paid). Escrow uses a different set.
// The screen has to know the rail before it offers a wallet button or a QR,
// because those controls move different money.

import { formatXmr, parseXmrPayment, XMR_CONFIRMATIONS } from './xmr.ts'

export const ORDER_STATUS = {
  awaitingPayment: 'Awaiting payment',
  paymentSeen: 'Payment seen',
  paid: 'Paid',
  awaitingTerms: 'Awaiting vendor terms',
  awaitingFunding: 'Awaiting funding',
  fundingSeen: 'Funding seen',
  funded: 'Funded',
  fundedWithoutRefund: 'Funded without a refund',
  released: 'Released',
  disputed: 'Disputed',
  settled: 'Settled by the moderator'
} as const

const ESCROW_STATUSES = new Set<string>([
  ORDER_STATUS.awaitingTerms,
  ORDER_STATUS.awaitingFunding,
  ORDER_STATUS.fundingSeen,
  ORDER_STATUS.funded,
  ORDER_STATUS.fundedWithoutRefund,
  ORDER_STATUS.released,
  ORDER_STATUS.disputed,
  ORDER_STATUS.settled
])

export type PayKind = 'btc' | 'xmr' | 'escrow'

type PayOrder = {
  settlement: string
  status: string
  escrow?: unknown
  payment?: { rail?: string } | null
}

export function isPaid(status: string | undefined): boolean {
  return status === ORDER_STATUS.paid
}

/** Stop polling a list once every order has confirmed, including an empty list. */
export function ordersSettled(orders: { status: string }[]): boolean {
  return orders.length === 0 || orders.every(order => isPaid(order.status))
}

export function payKind(order: PayOrder): PayKind {
  if (order.settlement === 'escrow' || order.escrow || ESCROW_STATUSES.has(order.status)) return 'escrow'
  if (order.payment?.rail === 'xmr') return 'xmr'
  return 'btc'
}

/** The bitcoin wallet button. Monero and escrow never take this path. */
export function canOfferBitcoinPay(
  order: { status: string; payTxid?: string | null },
  kind: PayKind,
  dueSats: number
): boolean {
  return kind === 'btc' && order.status === ORDER_STATUS.awaitingPayment && dueSats > 0 && !order.payTxid
}

/** Confirmations the status line should count toward. Monero always needs 10. */
export function confirmationTarget(
  kind: PayKind,
  moneroNeeded: number | null | undefined,
  bitcoinNeeded: number | undefined
): number | undefined {
  if (kind === 'xmr') return moneroNeeded ?? XMR_CONFIRMATIONS
  return bitcoinNeeded
}

export type MoneroDue = {
  network: 'stagenet' | 'mainnet'
  subaddress: string
  uri: string
  amountXmr: string
  receivedXmr: string
  remainingXmr: string
  overpaidXmr: string | null
  confirmationsNeeded: number
  awaiting: boolean
}

/** A Monero amount line from the node's flat payment object. Bitcoin payments are null. */
export function moneroDue(payment: unknown): MoneroDue | null {
  const fields = parseXmrPayment(payment)
  if (!fields) return null
  return {
    network: fields.network,
    subaddress: fields.subaddress,
    uri: fields.uri,
    amountXmr: formatXmr(fields.amount),
    receivedXmr: formatXmr(fields.received),
    remainingXmr: formatXmr(fields.remaining),
    overpaidXmr: fields.overpaid > 0n ? formatXmr(fields.overpaid) : null,
    confirmationsNeeded: fields.confirmationsNeeded,
    awaiting: fields.remaining > 0n
  }
}
