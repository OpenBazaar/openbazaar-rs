// Which payment controls an order may show.
//
// The node reports one status string for every direct rail
// (Awaiting payment -> Payment seen -> Paid). Escrow uses a different set.
// The screen has to know the rail before it offers a wallet button or a QR,
// because those controls move different money.

import { formatXmr, XMR_CONFIRMATIONS } from './xmr.ts'
import { formatZec, ZEC_CONFIRMATIONS } from './zec.ts'

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
  settled: 'Settled by the moderator',
  completed: 'Completed',
  declined: 'Declined by the seller'
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

export type PayKind = 'btc' | 'xmr' | 'zec' | 'escrow'

type PayOrder = {
  settlement: string
  status: string
  escrow?: unknown
  payment?: { rail?: string } | null
}

export function isPaid(status: string | undefined): boolean {
  return status === ORDER_STATUS.paid
}

/** Polling stops when a local order is paid, or a remote order is finished. */
export function orderPollingDone(order: { status: string; remote?: unknown }): boolean {
  if (order.remote) {
    return order.status === ORDER_STATUS.completed || order.status === ORDER_STATUS.declined
  }
  return isPaid(order.status)
}

export function ordersSettled(orders: { status: string; remote?: unknown }[]): boolean {
  return orders.length === 0 || orders.every(orderPollingDone)
}

export function payKind(order: PayOrder): PayKind {
  if (order.settlement === 'escrow' || order.escrow || ESCROW_STATUSES.has(order.status)) return 'escrow'
  if (order.payment?.rail === 'xmr') return 'xmr'
  if (order.payment?.rail === 'zec') return 'zec'
  return 'btc'
}

export function canOfferBitcoinPay(
  order: { status: string; payTxid?: string | null },
  kind: PayKind,
  dueSats: number
): boolean {
  return kind === 'btc' && order.status === ORDER_STATUS.awaitingPayment && dueSats > 0 && !order.payTxid
}

/** Confirmations an order needs: what its Monero or Zcash payment names, or the node's bitcoin setting. */
export function confirmationTarget(
  kind: PayKind,
  railNeeded: number | null | undefined,
  bitcoinNeeded: number | undefined
): number | undefined {
  if (kind === 'xmr') return railNeeded ?? XMR_CONFIRMATIONS
  if (kind === 'zec') return railNeeded ?? ZEC_CONFIRMATIONS
  return bitcoinNeeded
}

export type MoneroFields = {
  network: 'stagenet' | 'mainnet'
  subaddress: string
  uri: string
  amountPiconero: string
  receivedPiconero: string
  overpaidPiconero: string
  confirmationsNeeded: number
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

export function moneroDue(payment: { rail?: string; monero?: MoneroFields | null } | null | undefined): MoneroDue | null {
  const fields = payment?.rail === 'xmr' ? payment.monero : null
  if (!fields) return null
  let amount: bigint
  let received: bigint
  let overpaid: bigint
  try {
    amount = BigInt(fields.amountPiconero)
    received = BigInt(fields.receivedPiconero)
    overpaid = BigInt(fields.overpaidPiconero)
  } catch {
    return null
  }
  const remaining = amount > received ? amount - received : 0n
  return {
    network: fields.network,
    subaddress: fields.subaddress,
    uri: fields.uri,
    amountXmr: formatXmr(amount),
    receivedXmr: formatXmr(received),
    remainingXmr: formatXmr(remaining),
    overpaidXmr: overpaid > 0n ? formatXmr(overpaid) : null,
    confirmationsNeeded: fields.confirmationsNeeded,
    awaiting: remaining > 0n
  }
}

export type ZcashFields = {
  network: 'testnet' | 'mainnet'
  address: string
  uri: string
  amountZatoshi: string
  receivedZatoshi: string
  overpaidZatoshi: string
  confirmationsNeeded: number
}

export type ZcashDue = {
  network: 'testnet' | 'mainnet'
  address: string
  uri: string
  amountZec: string
  receivedZec: string
  remainingZec: string
  overpaidZec: string | null
  confirmationsNeeded: number
  awaiting: boolean
}

/** What a Zcash order still asks for. Received counts Ironwood notes only, as the node does. */
export function zcashDue(payment: { rail?: string; zcash?: ZcashFields | null } | null | undefined): ZcashDue | null {
  const fields = payment?.rail === 'zec' ? payment.zcash : null
  if (!fields) return null
  let amount: bigint
  let received: bigint
  let overpaid: bigint
  try {
    amount = BigInt(fields.amountZatoshi)
    received = BigInt(fields.receivedZatoshi)
    overpaid = BigInt(fields.overpaidZatoshi)
  } catch {
    return null
  }
  const remaining = amount > received ? amount - received : 0n
  return {
    network: fields.network,
    address: fields.address,
    // The node's URI asks for the whole amount. Once part has arrived, ask
    // for the rest (ZIP 321: the address and a decimal ZEC amount).
    uri: received > 0n && remaining > 0n ? `zcash:${fields.address}?amount=${formatZec(remaining)}` : fields.uri,
    amountZec: formatZec(amount),
    receivedZec: formatZec(received),
    remainingZec: formatZec(remaining),
    overpaidZec: overpaid > 0n ? formatZec(overpaid) : null,
    confirmationsNeeded: fields.confirmationsNeeded,
    awaiting: remaining > 0n
  }
}
