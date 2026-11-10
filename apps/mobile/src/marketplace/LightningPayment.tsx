import { useRef, useState, type FormEvent } from 'react'
import { fetchOrder, type BuyerOrder, type CurrencyCode, type RateTable, type SellerOrder } from '../catalog'
import {
  fetchLnStatus,
  issueLnTicket,
  lnNetworkLabel,
  payLnOrder,
  quoteLnPayment,
  type LnInvoice,
  type LnQuote,
  type LnTicket
} from '../lightning'
import { type LockStatus, isLocked } from '../walletLock'
import { useNodeResource } from '../useNodeResource'
import { CopyPaymentValue, PaymentQr, usePairedNode, WalletAmount } from './NodePayments'
import { RailMark } from './RailMark'
import { Dialog, FormError, Passphrase, UnlockDialog, useSubmit } from './WalletLock'

/** The Lightning part of the wallet page. Nothing while Lightning is off. */
export function LightningWalletCard({ rates, currency }: { rates: RateTable; currency: CurrencyCode }) {
  const { network } = usePairedNode()
  const status = useNodeResource(fetchLnStatus, 15000)
  const ln = status.data
  const showFiat = network.data?.network === 'mainnet'
  if (!ln?.enabled) return null
  return (
    <section className='ob-wallet-page ob-wallet-fund' aria-labelledby='ob-ln-wallet'>
      <h2 id='ob-ln-wallet' className='ob-rail'><RailMark rail='lightning' /> Lightning</h2>
      <p>{ln.network ? lnNetworkLabel(ln.network) : ''} · {ln.running ? `${ln.channels.length} channel${ln.channels.length === 1 ? '' : 's'}` : 'starting'}</p>
      {ln.balances && (
        <div className='ob-wallet-balances'>
          <article>
            <h2>Can spend</h2>
            <WalletAmount sats={ln.balances.lightningSats} rates={rates} currency={currency} showFiat={showFiat} />
          </article>
          <article className='is-pending'>
            <h2>Can receive</h2>
            <WalletAmount sats={ln.balances.inboundSats} rates={rates} currency={currency} showFiat={showFiat} />
          </article>
        </div>
      )}
      {ln.balances && ln.balances.onchainSats > 0 && (
        <p className='ob-wallet-note'>{ln.balances.onchainSats.toLocaleString()} sats sit in the Lightning node's on-chain wallet, from closed channels or for opening new ones.</p>
      )}
      {status.error && <p className='ob-form-error' role='alert'>{status.error}</p>}
    </section>
  )
}

/** Payment of one order on the Lightning rail: the invoice, and paying it from this node. */
export function LightningOrderPayment({ id, initial, invoice }: { id: string; initial: BuyerOrder | SellerOrder; invoice: LnInvoice }) {
  const { lock } = usePairedNode()
  const resource = useNodeResource(() => fetchOrder(id), 5000, true, o => o.status === 'Paid')
  const order = resource.data ?? initial
  const ln = order.payment?.ln ?? invoice
  const [step, setStep] = useState<{ kind: 'unlock' } | { kind: 'confirm'; quote: LnQuote } | null>(null)
  const [error, setError] = useState('')
  const submitting = useRef(false)
  const paid = order.status === 'Paid'
  const expired = ln.expired || ln.expiresAt * 1000 <= Date.now()
  const blocked = paid || expired || ln.local || order.paying || submitting.current
  async function start(status = lock.data) {
    if (blocked) return
    setError('')
    if (status?.state !== 'unlocked') {
      setStep({ kind: 'unlock' })
      return
    }
    submitting.current = true
    try {
      setStep({ kind: 'confirm', quote: await quoteLnPayment(id) })
    } catch (e) {
      if (isLocked(e)) {
        await lock.refresh()
        setStep({ kind: 'unlock' })
      } else setError(e instanceof Error ? e.message : 'The payment could not be quoted.')
    } finally {
      submitting.current = false
    }
  }
  function unlocked(status: LockStatus) {
    lock.update(status)
    setStep(null)
    if (status.state === 'unlocked') void start(status)
  }
  async function send(ticket: LnTicket) {
    setStep(null)
    submitting.current = true
    setError('')
    try {
      await payLnOrder(id, ticket.ticket, ticket.network === 'bitcoin')
    } catch (e) {
      if (isLocked(e)) void lock.refresh()
      setError(e instanceof Error ? e.message : 'The payment could not be started.')
    } finally {
      submitting.current = false
      await resource.refresh()
    }
  }
  return (
    <section className='ob-order-payment' aria-label={`Lightning payment for ${order.seller}`}>
      <div className='ob-payment-progress' aria-live='polite'>
        <strong>{order.status}</strong>
        <span className='ob-rail'><RailMark rail='lightning' size={16} /> {ln.amountSats.toLocaleString()} sats over Lightning</span>
        {paid && ln.lspFeeSats != null && ln.local && <span>The LSP kept {ln.lspFeeSats.toLocaleString()} sats to open the channel</span>}
      </div>
      {(error || order.payError || resource.error) && <p role='alert' className='ob-form-error'>{error || order.payError || resource.error}</p>}
      {order.paying && <p role='status'>Paying over Lightning…</p>}
      {!paid && expired && <p className='ob-form-error' role='alert'>This invoice has expired. Place the order again for a new one.</p>}
      {!paid && !expired && (
        <div className='ob-payment-options'>
          {!ln.local && (
            <div className='ob-pay-wallet'>
              <button className='ob-primary' disabled={blocked} onClick={() => void start()}>Pay from your node's Lightning balance</button>
              <p className='ob-pay-note'>This pays only {order.seller}. Routing fees are additional and capped.</p>
            </div>
          )}
          <div className='ob-wallet-receive'>
            <PaymentQr value={ln.uri} />
            <CopyPaymentValue label='Lightning invoice' value={ln.invoice} />
          </div>
          <p className='ob-page-intro'>
            {ln.local ? 'This node issued this invoice as the seller, so it cannot pay it. Pay it from another Lightning wallet. ' : 'Or pay this invoice from any Lightning wallet. '}
            It expires {new Date(ln.expiresAt * 1000).toLocaleTimeString(undefined, { timeStyle: 'short' })}.
            {ln.jit && ' The first payment opens a channel to the seller through their LSP.'}
          </p>
        </div>
      )}
      {step?.kind === 'unlock' && lock.data && <UnlockDialog status={lock.data} onCancel={() => setStep(null)} onUnlocked={unlocked} />}
      {step?.kind === 'confirm' && <LnConfirmDialog quote={step.quote} seller={order.seller} onCancel={() => setStep(null)} onTicket={ticket => void send(ticket)} />}
    </section>
  )
}

function LnConfirmDialog({ quote, seller, onCancel, onTicket }: { quote: LnQuote; seller: string; onCancel: () => void; onTicket: (ticket: LnTicket) => void }) {
  const [passphrase, setPassphrase] = useState('')
  const { busy, error, run } = useSubmit()
  const mainnet = quote.network === 'bitcoin'
  const title = mainnet ? 'Send real bitcoin over Lightning?' : 'Confirm Lightning payment'
  return (
    <Dialog label={title} onCancel={onCancel}>
      <form
        className='ob-spend-confirm'
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          void run(async () => onTicket(await issueLnTicket(quote.quote, passphrase)), 'The payment could not be confirmed.')
        }}
      >
        <h2>{title}</h2>
        <dl className='ob-spend-terms'>
          <dt>Pay</dt>
          <dd><strong>{quote.amountSats.toLocaleString()} sats</strong> to {seller}</dd>
          <dt>Routing fee</dt>
          <dd>At most {quote.feeCapSats.toLocaleString()} sats</dd>
          <dt>Network</dt>
          <dd>{lnNetworkLabel(quote.network)}</dd>
          <dt>Order</dt>
          <dd><code>{quote.orderId}</code></dd>
        </dl>
        <p className='ob-page-intro'>Your node pays only this invoice. Lightning payments cannot be reversed. This confirmation expires in {quote.expiresInSecs} seconds.</p>
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
