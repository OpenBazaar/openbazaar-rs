import { useState, type FormEvent } from 'react'
import type { NetworkName } from '../catalog'
import { bumpPayment } from '../nodeWallet'
import { isLocked, type LockStatus } from '../walletLock'
import { UnlockDialog } from './WalletLock'

/** A direct payment's vsize is small; this caps the fee at about this many vbytes. */
const CAP_VBYTES = 300

/** Replace an unconfirmed payment from this node with one at a higher fee. */
export function SpeedUpPayment({
  orderId,
  network,
  lock,
  onLock,
  onDone
}: {
  orderId: string
  network: NetworkName
  lock: LockStatus | null
  onLock: (status: LockStatus) => void
  onDone: () => void
}) {
  const [unlocking, setUnlocking] = useState(false)
  const [rate, setRate] = useState('3')
  const [passphrase, setPassphrase] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const feeRate = Number(rate)
  const maxFeeSats = Math.ceil(feeRate * CAP_VBYTES)
  async function submit(e?: FormEvent) {
    e?.preventDefault()
    if (busy) return
    setBusy(true)
    setError('')
    try {
      await bumpPayment(orderId, network, {
        feeRateSatVb: feeRate,
        maxFeeSats,
        passphrase
      })
      setPassphrase('')
      onDone()
    } catch (err) {
      if (isLocked(err) && lock) return setUnlocking(true)
      setError(
        err instanceof Error ? err.message : 'The fee could not be raised.'
      )
    } finally {
      setBusy(false)
    }
  }
  return (
    <details className='ob-external-payment'>
      <summary>Speed up payment</summary>
      <form onSubmit={e => void submit(e)} aria-label='Speed up payment'>
        <p className='ob-page-intro'>
          Replaces the unconfirmed payment with the same one at a higher fee.
          The seller gets the same amount; the extra fee comes from your
          change, up to {Number.isFinite(maxFeeSats) ? maxFeeSats : 0} sats.
        </p>
        <label className='ob-field'>
          <span>Fee rate (sat/vB)</span>
          <input
            type='number'
            min={1}
            max={500}
            step='any'
            value={rate}
            onChange={e => setRate(e.target.value)}
            required
          />
        </label>
        <label className='ob-field'>
          <span>Wallet passphrase</span>
          <input
            type='password'
            value={passphrase}
            onChange={e => setPassphrase(e.target.value)}
            autoComplete='current-password'
            required
          />
        </label>
        {error && (
          <p className='ob-form-error' role='alert'>
            {error}
          </p>
        )}
        <button className='ob-primary' disabled={busy || !(feeRate >= 1)}>
          {busy ? 'Sending…' : 'Raise the fee'}
        </button>
      </form>
      {unlocking && lock && (
        <UnlockDialog
          status={lock}
          reason='Unlock the wallet to raise the fee.'
          onCancel={() => setUnlocking(false)}
          onUnlocked={status => {
            onLock(status)
            setUnlocking(false)
            if (status.state === 'unlocked') void submit()
          }}
        />
      )}
    </details>
  )
}
