import { useRef, useState, type FormEvent } from 'react'
import {
  issueXmrTicket,
  payXmrOrder,
  quoteXmrPayment,
  type XmrQuote,
  type XmrTicket
} from '../xmr'
import {
  issueZecTicket,
  payZecOrder,
  quoteZecPayment,
  type ZecQuote,
  type ZecTicket
} from '../zec'
import { formatXmr } from '../xmr'
import { formatZec } from '../zec'
import { type LockStatus, isLocked } from '../walletLock'
import { usePairedNode } from './NodePayments'
import { Dialog, FormError, Passphrase, UnlockDialog, useSubmit } from './WalletLock'

type Rail = 'xmr' | 'zec'
type Quote = XmrQuote | ZecQuote

/** Pay one Monero or Zcash order from this node's wallet. */
export function ShieldedPay({
  id,
  seller,
  rail,
  paying,
  disabled,
  onDone
}: {
  id: string
  seller: string
  rail: Rail
  paying: boolean
  disabled: boolean
  onDone: () => void
}) {
  const { lock, network } = usePairedNode()
  const [step, setStep] = useState<{ kind: 'unlock' } | { kind: 'confirm'; quote: Quote } | null>(null)
  const [error, setError] = useState('')
  const submitting = useRef(false)
  const name = rail === 'xmr' ? 'Monero' : 'Zcash'
  async function start(status = lock.data) {
    if (disabled || paying || submitting.current) return
    setError('')
    if (status?.state !== 'unlocked') {
      setStep({ kind: 'unlock' })
      return
    }
    submitting.current = true
    try {
      setStep({ kind: 'confirm', quote: rail === 'xmr' ? await quoteXmrPayment(id) : await quoteZecPayment(id) })
    } catch (e) {
      if (isLocked(e)) {
        await lock.refresh()
        setStep({ kind: 'unlock' })
      } else setError(e instanceof Error ? e.message : `The ${name} payment could not be quoted.`)
    } finally {
      submitting.current = false
    }
  }
  function unlocked(status: LockStatus) {
    lock.update(status)
    setStep(null)
    if (status.state === 'unlocked') void start(status)
  }
  async function send(ticket: XmrTicket | ZecTicket) {
    setStep(null)
    submitting.current = true
    setError('')
    try {
      if (rail === 'xmr') await payXmrOrder(id, ticket.ticket, ticket.network === 'mainnet')
      else await payZecOrder(id, ticket.ticket, ticket.network === 'mainnet')
    } catch (e) {
      if (isLocked(e)) void lock.refresh()
      setError(e instanceof Error ? e.message : `The ${name} payment could not be started.`)
    } finally {
      submitting.current = false
      onDone()
    }
  }
  const quote = step?.kind === 'confirm' ? step.quote : null
  const backend = rail === 'xmr' ? network.data?.xmrBackend?.backend !== undefined && network.data.xmrBackend.backend !== 'none' : !!network.data?.zecBackend?.enabled
  if (!backend)
    return <p className='ob-pay-note'>This node's {name} wallet is off, so pay with another {name} wallet.</p>
  return (
    <>
      <div className='ob-pay-wallet'>
        <button className='ob-primary' type='button' disabled={disabled || paying} onClick={() => void start()}>
          Pay from your node wallet
        </button>
        <p className='ob-pay-note'>This pays only {seller} in {name}. The fee is included in the confirmation.</p>
      </div>
      {error && <p role='alert' className='ob-form-error'>{error}</p>}
      {step?.kind === 'unlock' && lock.data && <UnlockDialog status={lock.data} onCancel={() => setStep(null)} onUnlocked={unlocked} />}
      {quote && <Confirm quote={quote} seller={seller} rail={rail} onCancel={() => setStep(null)} onTicket={ticket => void send(ticket)} />}
    </>
  )
}

function Confirm({ quote, seller, rail, onCancel, onTicket }: { quote: Quote; seller: string; rail: Rail; onCancel: () => void; onTicket: (ticket: XmrTicket | ZecTicket) => void }) {
  const [passphrase, setPassphrase] = useState('')
  const { busy, error, run } = useSubmit()
  const mainnet = quote.network === 'mainnet'
  const amount = rail === 'xmr' ? `${formatXmr((quote as XmrQuote).amount)} XMR` : `${formatZec((quote as ZecQuote).amount)} ZEC`
  const fee = rail === 'xmr' ? `${formatXmr((quote as XmrQuote).feeCap)} XMR` : `${formatZec((quote as ZecQuote).feeCap)} ZEC`
  const title = mainnet ? `Send real ${rail === 'xmr' ? 'Monero' : 'Zcash'}?` : `Confirm ${rail === 'xmr' ? 'Monero' : 'Zcash'} payment`
  return (
    <Dialog label={title} onCancel={onCancel}>
      <form
        className='ob-spend-confirm'
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          void run(async () => {
            const ticket = rail === 'xmr' ? await issueXmrTicket(quote.quote, passphrase) : await issueZecTicket(quote.quote, passphrase)
            onTicket(ticket)
          }, 'The payment could not be confirmed.')
        }}
      >
        <h2>{title}</h2>
        <dl className='ob-spend-terms'>
          <dt>Pay</dt>
          <dd><strong>{amount}</strong> to {seller}</dd>
          <dt>Fee cap</dt>
          <dd>At most {fee}</dd>
          <dt>Network</dt>
          <dd>{quote.network}</dd>
          <dt>Order</dt>
          <dd><code>{quote.orderId}</code></dd>
        </dl>
        <p className='ob-page-intro'>Your node pays only this order. This confirmation expires in {quote.expiresInSecs} seconds.</p>
        <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} autoFocus />
        <FormError message={error} />
        <div className='ob-form-actions'>
          <button className='ob-secondary' type='button' onClick={onCancel}>Cancel</button>
          <button className='ob-primary' disabled={busy || !passphrase}>{busy ? 'Confirming…' : 'Confirm payment'}</button>
        </div>
      </form>
    </Dialog>
  )
}
