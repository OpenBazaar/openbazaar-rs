import { paired } from './catalog.ts'
import { verifyDisputeDecision, verifyFeeReceipt, type FeeReceiptRecord } from './records.ts'

// The filing fee on a v5 escrow: whoever opens a dispute pays the moderator
// outside the escrow, once the moderator has reviewed the letter and sent a
// signed request. The node pays only a quote the user confirmed with the
// wallet passphrase, and keeps the moderator's signed receipt.

export type FeeKind = 'filing' | 'ruling'

export type DisputeFeeState = 'awaiting-request' | 'requested' | 'paying' | 'paid' | 'denied' | 'recused'

/** Where this node's own filing fee stands, as the order or sale carries it. */
export type DisputeFee = {
  state: DisputeFeeState
  amountSats: number | null
  validUntilHeight: number | null
  dueHeight: number | null
  /** The newest request lapsed; the moderator sends another while there is time. */
  expired: boolean
  lightning: boolean
  rail: 'ln' | 'onchain' | null
  txid: string | null
  /** Hex of the moderator's signed receipt (`OBFR`), once paid. */
  receipt: string | null
  /** Hex of the moderator's signed denial (`OBDD`). */
  denial: string | null
  /** Hex of the moderator's signed recusal (`OBDD`), and the refund it sent this node. */
  recusal: string | null
  refundTxid: string | null
  /** On-chain payments this node cleared because they could not confirm. */
  clearedTxids: string[]
}

/** One payment of a fee request, as the node will make it. */
export type DisputeFeeQuote = {
  quote: string
  rail: 'ln' | 'onchain'
  amountSats: number
  /** The most the network or route may cost on top. */
  feeCapSats: number
  /** On-chain only: the fee this transaction pays and the total it spends. */
  feeSats: number | null
  totalSats: number | null
  address: string | null
  expiresInSecs: number
}

export type DisputeFeePaid = { rail: 'ln' | 'onchain'; amountSats: number; txid: string | null }

const STATES: readonly DisputeFeeState[] = ['awaiting-request', 'requested', 'paying', 'paid', 'denied', 'recused']

const record = (value: unknown) => (value && typeof value === 'object' && !Array.isArray(value) ? value : {}) as Record<string, unknown>
const count = (value: unknown) => (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null)
const text = (value: unknown) => (typeof value === 'string' && value ? value : null)
const rail = (value: unknown) => (value === 'ln' || value === 'onchain' ? value : null)

export function parseFeeKind(value: unknown): FeeKind | undefined {
  return value === 'filing' || value === 'ruling' ? value : undefined
}

/** `null` when the node sent no fee, as for a v4 escrow or a party that has not filed. */
export function parseDisputeFee(value: unknown): DisputeFee | null {
  const r = record(value)
  const state = STATES.find(known => known === r.state)
  if (!state) return null
  return {
    state,
    amountSats: count(r.amountSats),
    validUntilHeight: count(r.validUntilHeight),
    dueHeight: count(r.dueHeight),
    expired: r.expired === true,
    lightning: r.lightning === true,
    rail: rail(r.rail),
    txid: text(r.txid),
    receipt: text(r.receipt),
    denial: text(r.denial),
    recusal: text(r.recusal),
    refundTxid: text(r.refundTxid),
    clearedTxids: Array.isArray(r.clearedTxids) ? r.clearedTxids.flatMap(txid => text(txid) ?? []) : [],
  }
}

export function parseDisputeFeeQuote(value: unknown): DisputeFeeQuote {
  const r = record(value)
  const quote = text(r.quote), kind = rail(r.rail), amountSats = count(r.amountSats), feeCapSats = count(r.feeCapSats)
  if (!quote || !kind || amountSats === null || feeCapSats === null) throw new Error('The node returned an unreadable fee quote.')
  return {
    quote,
    rail: kind,
    amountSats,
    feeCapSats,
    feeSats: count(r.feeSats),
    totalSats: count(r.totalSats),
    address: text(r.address),
    expiresInSecs: count(r.expiresInSecs) ?? 0,
  }
}

/** The moderator's receipt for this fee, if its key signed it. */
export function verifiedReceipt(fee: DisputeFee, moderator: string): FeeReceiptRecord | null {
  return fee.receipt ? verifyFeeReceipt(fee.receipt, moderator) : null
}

/** Whether the moderator's key signed a denial of `filer`'s letter on this fee. */
export function verifiedDenial(fee: DisputeFee, moderator: string, filer: 'buyer' | 'vendor'): boolean {
  const decision = fee.denial ? verifyDisputeDecision(fee.denial, moderator) : null
  return decision?.decision === `denied-${filer}`
}

/** Whether the moderator's key signed a recusal from this case. */
export function verifiedRecusal(fee: DisputeFee, moderator: string): boolean {
  return (fee.recusal ? verifyDisputeDecision(fee.recusal, moderator) : null)?.decision === 'recused'
}

/** What the filer reads about its fee. `tip` is the chain height, when known. */
export function disputeFeeText(fee: DisputeFee, tip?: number): string {
  const sats = fee.amountSats === null ? 'the fee' : `${fee.amountSats.toLocaleString()} sats`
  const due = fee.dueHeight === null ? '' : ` A decision is due by block ${fee.dueHeight.toLocaleString()}.`
  switch (fee.state) {
    case 'awaiting-request':
      return 'Waiting for the moderator to review the letter and send its fee request.'
    case 'requested': {
      const cleared = fee.clearedTxids.length ? 'The earlier payment could not confirm, so it was cleared and its coins are back in the wallet. ' : ''
      return cleared + (fee.expired ? 'The fee request lapsed. The moderator sends a new one while there is time to decide.' : `The moderator reviewed the letter and asks ${sats} to take the case.`)
    }
    case 'paying':
      return `${sats} sent. Waiting for the moderator's receipt.`
    case 'paid':
      return tip !== undefined && fee.dueHeight !== null && tip > fee.dueHeight
        ? `Paid ${sats}. The decision is overdue: block ${fee.dueHeight.toLocaleString()} has passed. The fee is not refunded automatically.`
        : `Paid ${sats}; the moderator signed a receipt.${due}`
    case 'denied':
      return 'The moderator denied this letter. A denial is final, and the fee is not returned.'
    case 'recused':
      return fee.refundTxid
        ? `The moderator recused itself from this case and paid ${sats} back in transaction ${fee.refundTxid}. The escrow follows its timelocks.`
        : 'The moderator recused itself from this case. The escrow follows its timelocks.'
  }
}

/** What opening a dispute costs, before it is filed. */
export function filingFeeNote(kind: FeeKind | undefined, sats: number | undefined): string {
  if (kind !== 'filing') return sats ? `The moderator's fee of ${sats.toLocaleString()} sats comes out of the escrow only if it rules.` : ''
  if (!sats) return 'This moderator charges no filing fee.'
  return `Opening a dispute costs ${sats.toLocaleString()} sats, paid to the moderator once it has reviewed your letter. A denial is final and the fee is not returned.`
}

const post = (path: string, body: object) =>
  paired(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })

export async function openDispute(orderId: string, memo: string): Promise<void> {
  await post(`/orders/${encodeURIComponent(orderId)}/dispute`, { memo })
}

export async function quoteDisputeFee(orderId: string): Promise<DisputeFeeQuote> {
  return parseDisputeFeeQuote(await post(`/orders/${encodeURIComponent(orderId)}/dispute-fee/quote`, {}))
}

/** Pay a quote the user confirmed. The node takes it once. */
export async function payDisputeFee(orderId: string, quote: string, passphrase: string): Promise<DisputeFeePaid> {
  const r = record(await post(`/orders/${encodeURIComponent(orderId)}/dispute-fee`, { quote, passphrase, confirm: true }))
  const kind = rail(r.rail), amountSats = count(r.amountSats)
  if (!kind || amountSats === null) throw new Error('The node returned an unreadable fee payment.')
  return { rail: kind, amountSats, txid: text(r.txid) }
}
