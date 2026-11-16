import { useState } from 'react'
import { lnNetworkLabel, type LnNetwork, type LnStatus } from '../lightning'
import { lnAddressProblem, lnInvoiceProblem, payLnInvoice, quoteLnInvoice, quoteLnWithdraw, withdrawLn, type LnInvoiceQuote, type LnWithdrawQuote } from '../lnSend'
import { parseSats } from '../walletSend'
import { CopyAddress } from './CopyAddress'
import { usePairedNode } from './NodePayments'
import { SendDialog, type SendRail } from './SendDialog'

const sats = (value: number) => `${value.toLocaleString('en-US')} sats`
const IRREVERSIBLE = 'Your node will send exactly this and nothing else. It cannot be reversed.'

export function invoiceRail(network: LnNetwork): SendRail<LnInvoiceQuote> {
  return {
    rail: 'lightning',
    title: 'Pay a Lightning invoice',
    network: lnNetworkLabel(network),
    destination: { label: 'Lightning invoice', placeholder: network === 'signet' ? 'lntbs1…' : 'ln…', multiline: true },
    check: input => lnInvoiceProblem(input.destination, network),
    quote: input => quoteLnInvoice(input.destination),
    review: quote => ({
      rows: [
        ['Pay', <strong>{sats(quote.amountSats)}</strong>],
        ['Description', quote.description || 'None'],
        ['Payee', <code title={quote.payee}>{quote.payee.slice(0, 12)}…{quote.payee.slice(-8)}</code>],
        ['Routing fee', `At most ${sats(quote.feeCapSats)}`],
        ['Total', `At most ${sats(quote.amountSats + quote.feeCapSats)}`],
        ['Network', lnNetworkLabel(quote.network)]
      ],
      note: IRREVERSIBLE,
      expiresInSecs: quote.expiresInSecs
    }),
    send: async (quote, passphrase) => {
      const paid = await payLnInvoice(quote.quote, passphrase)
      if (paid.status === 'failed') throw new Error('The payment did not go through, so nothing was paid. Try again later or ask for a new invoice.')
      return {
        idLabel: 'Payment hash',
        id: paid.paymentHash,
        summary: paid.status === 'paid'
          ? `Paid ${sats(paid.amountSats)}${paid.feeSats ? ` with a ${sats(paid.feeSats)} routing fee` : ''}.`
          : `Sending ${sats(paid.amountSats)}. The payment is still settling.`
      }
    },
    sendLabel: 'Pay invoice'
  }
}

export function withdrawRail(network: LnNetwork): SendRail<LnWithdrawQuote> {
  const name = network === 'signet' ? 'Mutinynet' : lnNetworkLabel(network).split(' · ')[0]
  return {
    rail: 'lightning',
    title: 'Withdraw on-chain funds',
    network: lnNetworkLabel(network),
    destination: {
      label: `Recipient address on ${name}`,
      placeholder: network === 'bitcoin' ? 'bc1q…' : 'tb1q…',
      help: `Sends the Lightning node's on-chain funds, such as those returned from closed channels, to an address on ${name}.${network === 'signet' ? ' Your bitcoin wallet is on testnet4, so it cannot receive them.' : ''}`
    },
    amount: { label: 'Amount in sats', unit: 'sats', max: true },
    check: input => lnAddressProblem(input.destination, network)
      ?? (input.max || parseSats(input.amount) ? null : 'Enter a whole number of sats above zero.'),
    quote: input => quoteLnWithdraw(input.destination, input.max ? undefined : parseSats(input.amount) ?? undefined),
    review: quote => ({
      rows: [
        ['Withdraw', quote.max ? <><strong>Everything on chain</strong> (now {sats(quote.amountSats)})</> : <strong>{sats(quote.amountSats)}</strong>],
        ['To', <CopyAddress value={quote.address} label='Copy recipient address'/>],
        ['Network fee', quote.max ? 'Set by the Lightning node when it sends, and taken from the amount. It keeps a small reserve for its channels.' : 'Set by the Lightning node when it sends, and paid on top of the amount.'],
        ['Network', lnNetworkLabel(quote.network)]
      ],
      note: IRREVERSIBLE,
      expiresInSecs: quote.expiresInSecs
    }),
    send: async (quote, passphrase) => {
      const sent = await withdrawLn(quote.quote, passphrase)
      return { idLabel: 'Transaction ID', id: sent.txid, summary: sent.max ? 'Withdrew the on-chain funds.' : `Withdrew ${sats(sent.amountSats)}.` }
    },
    sendLabel: 'Withdraw'
  }
}

/** The Lightning wallet's Pay invoice and Withdraw buttons, and their dialogs. Lightning never runs on mainnet. */
export function LightningSendButtons({ ln }: { ln: LnStatus }) {
  const { lock } = usePairedNode()
  const [open, setOpen] = useState<'pay' | 'withdraw' | null>(null)
  const network = ln.network
  if (!network || network === 'bitcoin') return null
  const close = () => setOpen(null)
  return <>
    <button type='button' className='ob-secondary' disabled={!ln.running || !ln.balances?.lightningSats} onClick={() => setOpen('pay')}>Pay invoice</button>
    <button type='button' className='ob-secondary' disabled={!ln.running || !ln.balances?.onchainSats} onClick={() => setOpen('withdraw')}>Withdraw</button>
    {open === 'pay' && <SendDialog config={invoiceRail(network)} lock={lock} onCancel={close} onSent={() => undefined}/>}
    {open === 'withdraw' && <SendDialog config={withdrawRail(network)} lock={lock} onCancel={close} onSent={() => undefined}/>}
  </>
}
