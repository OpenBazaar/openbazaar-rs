import type { CurrencyCode } from '../catalog'
import { fiatView, type CoinRate } from '../walletFiat'

/** "≈ $3.42", with any test-coin or staleness note, under a wallet amount. */
export function FiatEstimate({ coins, rate, currency, testCoins }: { coins: number; rate: CoinRate | null | undefined; currency: CurrencyCode; testCoins: boolean }) {
  const { estimate, note } = fiatView({ coins, rate, currency, testCoins })
  return <span className='ob-wallet-fiat'>{estimate}{estimate && note && ' · '}{note && <span className='ob-wallet-fiat-note'>{note}</span>}</span>
}
