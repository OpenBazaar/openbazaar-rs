import { useState } from 'react'
import type { NetworkName } from '../catalog'
import { bitcoinAmount } from '../nodeWallet'
import { bitcoinAddressProblem, parseSats, quoteBitcoinSend, sendBitcoin, type BitcoinSendQuote } from '../walletSend'
import { CopyAddress } from './CopyAddress'
import { usePairedNode } from './NodePayments'
import { SendDialog, type SendRail } from './SendDialog'

const sats = (value: number) => `${value.toLocaleString('en-US')} sats`

export function bitcoinRail(network: NetworkName): SendRail<BitcoinSendQuote> {
  const mainnet = network === 'mainnet'
  return {
    rail: 'bitcoin',
    title: 'Send bitcoin',
    network: mainnet ? 'Bitcoin mainnet · real bitcoin' : 'Testnet4 · test coins, no value',
    destination: { label: 'Recipient address', placeholder: mainnet ? 'bc1q…' : 'tb1q…' },
    amount: { label: 'Amount in sats', unit: 'sats', max: true },
    linkable: true,
    check: input => bitcoinAddressProblem(input.destination, network)
      ?? (input.max || parseSats(input.amount) ? null : 'Enter a whole number of sats above zero.'),
    quote: input => quoteBitcoinSend({
      address: input.destination,
      amountSats: input.max ? undefined : parseSats(input.amount) ?? undefined,
      linkCoins: input.linkCoins
    }),
    review: quote => ({
      rows: [
        ['Send', <><strong>{sats(quote.amountSats)}</strong> ({bitcoinAmount(quote.amountSats)} BTC){quote.max ? ' · everything you can spend' : ''}</>],
        ['To', <CopyAddress value={quote.address} label='Copy recipient address'/>],
        ['Network fee', `${sats(quote.feeSats)} at ${quote.feeRateSatVb} sat/vB (never more than ${sats(quote.feeCapSats)})`],
        ['Total', sats(quote.totalSats)],
        ['Network', mainnet ? 'Mainnet' : 'Testnet4 · test coins, no value']
      ],
      note: mainnet
        ? 'Your node will send exactly this and nothing else. Bitcoin sends cannot be reversed.'
        : 'Your node will send exactly this and nothing else. Bitcoin sends cannot be reversed, even on testnet4.',
      heldBackSats: quote.heldBackSats,
      expiresInSecs: quote.expiresInSecs
    }),
    send: async (quote, passphrase) => {
      const sent = await sendBitcoin(quote.quote, passphrase)
      return {
        idLabel: 'Transaction ID',
        id: sent.txid,
        summary: `Sent ${sats(sent.amountSats)} with a ${sats(sent.feeSats)} network fee.`,
        note: 'It shows as pending in wallet activity until it confirms.'
      }
    },
    sendLabel: mainnet ? 'Send real bitcoin' : 'Send test bitcoin'
  }
}

/** The bitcoin wallet's Send button and its dialog. */
export function BitcoinSendButton() {
  const { wallet, network, lock } = usePairedNode()
  const [open, setOpen] = useState(false)
  const current = wallet.data
  const mismatch = !!current && !!network.data && current.network !== network.data.network
  const empty = !!current && current.balance.spendableSats === 0
  const blocked = !current || mismatch || empty || !!wallet.error || !!network.error
  return <>
    <button type='button' className='ob-secondary' disabled={blocked} title={empty ? 'Nothing to send yet.' : undefined} onClick={() => setOpen(true)}>Send</button>
    {open && current && <SendDialog config={bitcoinRail(current.network)} lock={lock} onCancel={() => setOpen(false)} onSent={() => void wallet.refresh()}/>}
  </>
}
