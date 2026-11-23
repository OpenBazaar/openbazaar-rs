import { useState } from 'react'
import { formatZec } from '../zec'
import { parseZecAmount, quoteZecSend, sendZec, zecAddressProblem, type ZecSendQuote } from '../zecSend'
import { CopyAddress } from './CopyAddress'
import { usePairedNode } from './NodePayments'
import { SendDialog, type SendRail } from './SendDialog'

const zec = (zatoshi: bigint) => `${formatZec(zatoshi)} ZEC`

export function zcashRail(): SendRail<ZecSendQuote> {
  return {
    rail: 'zcash',
    title: 'Send Zcash',
    network: 'Testnet · test coins, no value',
    destination: { label: 'Recipient address', placeholder: 'utest1…', multiline: true },
    amount: { label: 'Amount in ZEC', unit: 'ZEC', max: false },
    check: input => zecAddressProblem(input.destination, 'testnet')
      ?? (parseZecAmount(input.amount) ? null : 'Enter an amount in ZEC above zero, with at most 8 decimals.'),
    quote: input => quoteZecSend(input.destination, parseZecAmount(input.amount) ?? 0n),
    review: quote => ({
      rows: [
        ['Send', <strong>{zec(quote.amount)}</strong>],
        ['To', <CopyAddress value={quote.address} label='Copy recipient address'/>],
        ['Network fee', `${zec(quote.fee)} (never more than ${zec(quote.feeCap)})`],
        ['Total', zec(quote.total)],
        ['Network', 'Testnet · test coins, no value']
      ],
      note: 'Your node will send exactly this and nothing else. Zcash sends cannot be reversed. The notes spent are the same confirmed Ironwood notes an order payment uses, and the change is spendable again after 30 confirmations.',
      expiresInSecs: quote.expiresInSecs
    }),
    send: async (quote, passphrase) => {
      const sent = await sendZec(quote.quote, passphrase)
      return {
        idLabel: 'Transaction ID',
        id: sent.txid,
        summary: `Sent ${zec(sent.amount)} with a ${zec(sent.fee)} network fee.`,
        note: 'The balance updates at the next Zcash sync.'
      }
    },
    sendLabel: 'Send test Zcash'
  }
}

/** The Zcash wallet's Send button and its dialog. Testnet only. */
export function ZcashSendButton({ blocked, onSent }: { blocked: boolean; onSent: () => void }) {
  const { lock } = usePairedNode()
  const [open, setOpen] = useState(false)
  return <>
    <button type='button' className='ob-secondary' disabled={blocked} onClick={() => setOpen(true)}>Send</button>
    {open && <SendDialog config={zcashRail()} lock={lock} onCancel={() => setOpen(false)} onSent={onSent}/>}
  </>
}
