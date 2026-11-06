import type { NetworkName } from './catalog'

// The node does not expose a fee estimate, so the client keeps this much back
// for the network fee before it offers to pay from the node wallet. It covers
// a one-recipient payment of a few inputs at a generous rate for each network.
// The node still builds the transaction and refuses if the real fee does not fit.
export const FEE_RESERVE_SATS: Record<NetworkName, number> = {
  testnet4: 500,
  mainnet: 10_000
}

export type PaymentStatusInput = {
  network: NetworkName
  status: string
  confirmations: number
  requiredConfirmations?: number
  dueSats: number
  sending: boolean
  payTxid?: string | null
  wallet?: { spendableSats: number; totalSats: number } | null
}

/** Why the node wallet cannot pay yet. `pending` clears once coins confirm. */
export type BalanceNote = { kind: 'pending' | 'short'; message: string }

export type PaymentStatusView = {
  confirmations: string
  progress: string | null
  balance: BalanceNote | null
}

const sats = (value: number) => value.toLocaleString('en-US')

export function paymentStatusView(input: PaymentStatusInput): PaymentStatusView {
  return {
    confirmations: confirmationsText(input),
    progress: progressText(input),
    balance: balanceNote(input)
  }
}

function confirmationsText({ status, confirmations, requiredConfirmations }: PaymentStatusInput): string {
  if (status === 'Paid') return 'Confirmed'
  return requiredConfirmations === undefined
    ? `Confirmations: ${confirmations}`
    : `Confirmations: ${confirmations} / ${requiredConfirmations}`
}

function progressText({ status, confirmations, requiredConfirmations, sending, payTxid }: PaymentStatusInput): string | null {
  if (sending) return 'Sending payment from your node wallet…'
  if (status === 'Paid') return null
  if (status === 'Payment seen') {
    if (requiredConfirmations === undefined) return 'Payment seen. Waiting for confirmations.'
    const left = Math.max(1, requiredConfirmations - confirmations)
    return `Payment seen. Waiting for ${left} more confirmation${left === 1 ? '' : 's'}.`
  }
  if (payTxid) return 'Payment sent. Waiting for the node to see it on the chain.'
  return null
}

function balanceNote({ network, dueSats, wallet }: PaymentStatusInput): BalanceNote | null {
  if (!wallet || dueSats <= 0) return null
  const reserve = FEE_RESERVE_SATS[network]
  const needed = dueSats + reserve
  if (wallet.spendableSats >= needed) return null
  // Coins from other wallets count only once they confirm; the wallet's own
  // unconfirmed change is already part of the spendable balance.
  const unconfirmed = Math.max(0, wallet.totalSats - wallet.spendableSats)
  if (wallet.spendableSats + unconfirmed >= needed)
    return {
      kind: 'pending',
      message: `${sats(unconfirmed)} sats are waiting for a confirmation. You can pay once they confirm.`
    }
  const waiting = unconfirmed > 0 ? ` ${sats(unconfirmed)} more sats are waiting for a confirmation.` : ''
  return {
    kind: 'short',
    message: `Your wallet has ${sats(wallet.spendableSats)} spendable sats. This payment needs about ${sats(needed)}, including up to ${sats(reserve)} sats kept for the network fee.${waiting}`
  }
}
