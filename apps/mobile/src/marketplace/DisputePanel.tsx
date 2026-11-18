import { useState, type FormEvent } from 'react'
import type { EscrowReceipt } from '../catalog'
import {
  disputeFeeText,
  filingFeeNote,
  openDispute,
  payDisputeFee,
  quoteDisputeFee,
  verifiedDenial,
  verifiedRecusal,
  verifiedReceipt,
  type DisputeFee,
  type DisputeFeeQuote
} from '../disputeFee'
import { ORDER_STATUS } from '../orderState'
import { Dialog, FormError, Passphrase, useSubmit } from './WalletLock'

const DISPUTABLE = new Set<string>([ORDER_STATUS.funded, ORDER_STATUS.fundedWithoutRefund])

/** The buyer's side of a dispute: the letter, then the filing fee the moderator asks for. */
export function DisputePanel({ id, status, escrow, onChange }: { id: string; status: string; escrow: EscrowReceipt; onChange: () => void }) {
  const [memo, setMemo] = useState('')
  const [quote, setQuote] = useState<DisputeFeeQuote | null>(null)
  const { busy, error, run } = useSubmit()
  const fee = escrow.disputeFee ?? null
  const disputed = status === ORDER_STATUS.disputed
  if (!disputed && !DISPUTABLE.has(status)) return null
  if (!disputed) {
    const note = filingFeeNote(escrow.feeKind, escrow.moderatorFeeSats)
    return (
      <details className='ob-dispute-panel'>
        <summary>Open a dispute</summary>
        <form onSubmit={(e: FormEvent) => { e.preventDefault(); void run(async () => { await openDispute(id, memo.trim()); onChange() }, 'The dispute could not be sent.') }}>
          <p className='ob-page-intro'>Your letter goes to the moderator in the escrow terms. It can rule that the escrow pays you back.{note && ` ${note}`}</p>
          <label className='ob-field'>What went wrong<textarea value={memo} maxLength={1000} required onChange={e => setMemo(e.target.value)} placeholder='What arrived, what did not, and what you and the seller said.'/></label>
          <FormError message={error}/>
          <div className='ob-form-actions'><button className='ob-primary' disabled={busy || !memo.trim()}>{busy ? 'Sending…' : 'Send dispute letter'}</button></div>
        </form>
      </details>
    )
  }
  if (!fee) return escrow.feeKind === 'filing' ? <p className='ob-page-intro' role='status'>Dispute sent. Waiting for the moderator to review it.</p> : null
  return (
    <section className='ob-dispute-panel' aria-label='Dispute filing fee'>
      <p className='ob-page-intro' role='status'>{disputeFeeText(fee)}</p>
      <FeeProof fee={fee} moderator={escrow.moderator?.id}/>
      {fee.state === 'requested' && !fee.expired && (
        <>
          <FormError message={quote ? '' : error}/>
          <button className='ob-primary' disabled={busy} onClick={() => void run(async () => setQuote(await quoteDisputeFee(id)), 'The fee could not be quoted.')}>
            {busy && !quote ? 'Checking…' : `Pay ${fee.amountSats?.toLocaleString() ?? 'the'} sats filing fee`}
          </button>
        </>
      )}
      {quote && <FeeConfirmDialog id={id} quote={quote} moderator={escrow.moderator?.name || 'the moderator'} onCancel={() => setQuote(null)} onPaid={() => { setQuote(null); onChange() }}/>}
    </section>
  )
}

function FeeProof({ fee, moderator }: { fee: DisputeFee; moderator?: string }) {
  if (!moderator) return null
  if (fee.receipt) {
    const receipt = verifiedReceipt(fee, moderator)
    return receipt
      ? <p className='ob-page-intro'>Receipt signed by the moderator’s key: {receipt.receivedSats.toLocaleString()} sats received at block {receipt.paidHeight.toLocaleString()}, decision due by block {receipt.dueHeight.toLocaleString()}.</p>
      : <p className='ob-form-error' role='alert'>The receipt this node holds is not signed by the moderator in the escrow terms.</p>
  }
  if (fee.denial && !verifiedDenial(fee, moderator, 'buyer')) return <p className='ob-form-error' role='alert'>The denial this node holds is not signed by the moderator in the escrow terms.</p>
  if (fee.recusal && !verifiedRecusal(fee, moderator)) return <p className='ob-form-error' role='alert'>The recusal this node holds is not signed by the moderator in the escrow terms.</p>
  return null
}

function FeeConfirmDialog({ id, quote, moderator, onCancel, onPaid }: { id: string; quote: DisputeFeeQuote; moderator: string; onCancel: () => void; onPaid: () => void }) {
  const [passphrase, setPassphrase] = useState('')
  const { busy, error, run } = useSubmit()
  const title = 'Pay the filing fee'
  return (
    <Dialog label={title} onCancel={onCancel}>
      <form className='ob-spend-confirm' onSubmit={(e: FormEvent) => { e.preventDefault(); void run(async () => { await payDisputeFee(id, quote.quote, passphrase); onPaid() }, 'The fee was not paid.') }}>
        <h2>{title}</h2>
        <dl className='ob-spend-terms'>
          <dt>Pay</dt>
          <dd><strong>{quote.amountSats.toLocaleString()} sats</strong> to {moderator}</dd>
          <dt>Rail</dt>
          <dd>{quote.rail === 'ln' ? 'Lightning' : 'Bitcoin, on-chain'}</dd>
          <dt>{quote.rail === 'ln' ? 'Routing fee' : 'Network fee'}</dt>
          <dd>{quote.feeSats !== null ? `${quote.feeSats.toLocaleString()} sats` : `At most ${quote.feeCapSats.toLocaleString()} sats`}</dd>
          {quote.totalSats !== null && <><dt>Total</dt><dd>{quote.totalSats.toLocaleString()} sats</dd></>}
        </dl>
        <p className='ob-page-intro'>The fee is paid outside the escrow and is not returned, whatever the moderator decides. This confirmation expires in {quote.expiresInSecs} seconds.</p>
        <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} autoFocus/>
        <FormError message={error}/>
        <div className='ob-form-actions'>
          <button className='ob-secondary' type='button' onClick={onCancel}>Cancel</button>
          <button className='ob-primary' disabled={busy || !passphrase}>{busy ? 'Paying…' : 'Confirm payment'}</button>
        </div>
      </form>
    </Dialog>
  )
}
