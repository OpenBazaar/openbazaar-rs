import { useState } from 'react'
import { formatXmr, type XmrNetwork } from '../xmr'
import { parseXmrAmount, quoteXmrSend, sendXmr, xmrAddressProblem, type XmrSendQuote } from '../xmrSend'
import { CopyAddress } from './CopyAddress'
import { usePairedNode } from './NodePayments'
import { SendDialog, type SendRail } from './SendDialog'

const xmr = (piconero: bigint) => `${formatXmr(piconero)} XMR`

export function moneroRail(network: XmrNetwork): SendRail<XmrSendQuote> {
  const mainnet = network === 'mainnet'
  return {
    rail: 'monero',
    title: 'Send Monero',
    network: mainnet ? 'Monero mainnet · real Monero' : 'Stagenet · test coins, no value',
    destination: { label: 'Recipient address', placeholder: mainnet ? '4… or 8…' : '5… or 7…', multiline: true },
    amount: { label: 'Amount in XMR', unit: 'XMR', max: false },
    check: input => xmrAddressProblem(input.destination, network)
      ?? (parseXmrAmount(input.amount) ? null : 'Enter an amount in XMR above zero, with at most 12 decimals.'),
    quote: input => quoteXmrSend(input.destination, parseXmrAmount(input.amount) ?? 0n),
    review: quote => ({
      rows: [
        ['Send', <strong>{xmr(quote.amount)}</strong>],
        ['To', <CopyAddress value={quote.address} label='Copy recipient address'/>],
        ['Network fee', `${xmr(quote.fee)} (never more than ${xmr(quote.feeCap)})`],
        ['Total', xmr(quote.total)],
        ['Network', mainnet ? 'Mainnet' : 'Stagenet · test coins, no value']
      ],
      note: 'Your node will send exactly this and nothing else. Monero sends cannot be reversed, and the change is spendable again after 10 confirmations.',
      expiresInSecs: quote.expiresInSecs
    }),
    send: async (quote, passphrase) => {
      const sent = await sendXmr(quote.quote, passphrase)
      return {
        idLabel: 'Transaction ID',
        id: sent.txid,
        summary: `Sent ${xmr(sent.amount)} with a ${xmr(sent.fee)} network fee.`,
        note: 'The balance updates at the next Monero sync.'
      }
    },
    sendLabel: mainnet ? 'Send real Monero' : 'Send test Monero'
  }
}

/** The Monero wallet's Send button and its dialog. */
export function MoneroSendButton({ network, blocked, onSent }: { network: XmrNetwork; blocked: boolean; onSent: () => void }) {
  const { lock } = usePairedNode()
  const [open, setOpen] = useState(false)
  return <>
    <button type='button' className='ob-secondary' disabled={blocked} onClick={() => setOpen(true)}>Send</button>
    {open && <SendDialog config={moneroRail(network)} lock={lock} onCancel={() => setOpen(false)} onSent={onSent}/>}
  </>
}
