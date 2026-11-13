import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { shopPairingMessage } from '../catalog'
import { nodeTarget, servedByHomeNode } from '../homeNode/transport'
import { bitcoinAmount } from '../nodeWallet'
import { useNodeResource } from '../useNodeResource'
import { CopyAddress } from './CopyAddress'
import {
  confirmBackup,
  fetchLock,
  issueTicket,
  lockWallet,
  passphraseError,
  reportActivity,
  revealPhrase,
  setupWallet,
  unlockRemembered,
  unlockWallet,
  updateLockSettings,
  type LockStatus,
  type Quote,
  type Ticket
} from '../walletLock'

const ACTIVITY_PING_MS = 30_000
const AUTO_LOCK_CHOICES = [1, 5, 15, 30, 60]

/** The node's lock status, plus activity pings while the user is at the app. */
export function useLockState() {
  const lock = useNodeResource(fetchLock, 10000)
  const lastPing = useRef(0)
  const unlocked = lock.data?.state === 'unlocked'
  const { update } = lock
  useEffect(() => {
    if (!unlocked || typeof window === 'undefined') return
    const ping = () => {
      const now = Date.now()
      if (now - lastPing.current < ACTIVITY_PING_MS) return
      lastPing.current = now
      void reportActivity().then(update, () => undefined)
    }
    window.addEventListener('pointerdown', ping)
    window.addEventListener('keydown', ping)
    return () => {
      window.removeEventListener('pointerdown', ping)
      window.removeEventListener('keydown', ping)
    }
  }, [unlocked, update])
  return lock
}
export type LockResource = ReturnType<typeof useLockState>

export function Passphrase({
  label,
  value,
  onChange,
  autoFocus = false,
  autoComplete = 'current-password'
}: {
  label: string
  value: string
  onChange: (value: string) => void
  autoFocus?: boolean
  autoComplete?: string
}) {
  return (
    <label className='ob-field'>
      <span>{label}</span>
      <input
        type='password'
        value={value}
        autoFocus={autoFocus}
        data-dialog-autofocus={autoFocus || undefined}
        autoComplete={autoComplete}
        onChange={e => onChange(e.target.value)}
      />
    </label>
  )
}

export function useSubmit() {
  const [busy, setBusy] = useState(false),
    [error, setError] = useState('')
  async function run(task: () => Promise<void>, fallback: string) {
    if (busy) return
    setBusy(true)
    setError('')
    try {
      await task()
    } catch (e) {
      setError(passphraseError(e, fallback))
    } finally {
      setBusy(false)
    }
  }
  return { busy, error, run }
}

export function FormError({ message }: { message: string }) {
  return message ? (
    <p className='ob-form-error' role='alert'>
      {message}
    </p>
  ) : null
}

/** Shows the phrase, then asks for the words at `positions`. */
function BackupCheck({
  phrase,
  positions,
  onDone
}: {
  phrase: string[]
  positions: number[]
  onDone: (status: LockStatus | null) => void
}) {
  const [written, setWritten] = useState(false)
  const [words, setWords] = useState(() => positions.map(() => ''))
  const { busy, error, run } = useSubmit()
  if (!written)
    return (
      <div className='ob-wallet-lock'>
        <h2>Write down your backup phrase</h2>
        <p>
          These {phrase.length} words restore this wallet if you forget the
          passphrase or lose this device. Anyone with them can spend your
          bitcoin. They are shown here once.
        </p>
        <ol className='ob-backup-phrase'>
          {phrase.map((word, i) => (
            <li key={i}>{word}</li>
          ))}
        </ol>
        <div className='ob-form-actions'>
          <button
            className='ob-primary'
            onClick={() =>
              positions.length ? setWritten(true) : onDone(null)
            }
          >
            I wrote it down
          </button>
        </div>
      </div>
    )
  return (
    <form
      className='ob-wallet-lock'
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        void run(
          async () => onDone(await confirmBackup(words.map(w => w.trim()))),
          'The words could not be checked.'
        )
      }}
    >
      <h2>Confirm your backup</h2>
      {positions.map((position, i) => (
        <label className='ob-field' key={position}>
          <span>Word {position}</span>
          <input
            autoComplete='off'
            autoCapitalize='none'
            spellCheck={false}
            value={words[i]}
            onChange={e =>
              setWords(w => w.map((v, j) => (j === i ? e.target.value : v)))
            }
          />
        </label>
      ))}
      <FormError message={error} />
      <div className='ob-form-actions'>
        <button className='ob-secondary' type='button' onClick={() => setWritten(false)}>
          Show the words again
        </button>
        <button className='ob-primary' disabled={busy}>
          {busy ? 'Checking…' : 'Confirm backup'}
        </button>
      </div>
    </form>
  )
}

export function SetupWallet({ status, onStatus }: { status: LockStatus; onStatus: (s: LockStatus) => void }) {
  const [passphrase, setPassphrase] = useState(''),
    [repeat, setRepeat] = useState('')
  const [backup, setBackup] = useState<{ phrase: string[]; confirmWords: number[]; lock: LockStatus } | null>(null)
  const { busy, error, run } = useSubmit()
  if (backup)
    return (
      <BackupCheck
        phrase={backup.phrase}
        positions={backup.confirmWords}
        onDone={next => onStatus(next ?? backup.lock)}
      />
    )
  const short = passphrase.length < status.minPassphraseChars
  return (
    <form
      className='ob-wallet-lock'
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        if (short || passphrase !== repeat) return
        void run(async () => {
          const created = await setupWallet(passphrase)
          setPassphrase('')
          setRepeat('')
          setBackup(created)
        }, 'The wallet passphrase could not be set.')
      }}
    >
      <h2>Protect this wallet</h2>
      <p>
        Choose a wallet passphrase. Your node asks for it to unlock the wallet
        and again before every payment. Until it is set, this node will not
        spend.
      </p>
      <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} autoFocus autoComplete='new-password' />
      <Passphrase label='Repeat passphrase' value={repeat} onChange={setRepeat} autoComplete='new-password' />
      {passphrase && short && (
        <p className='ob-page-intro'>Use at least {status.minPassphraseChars} characters.</p>
      )}
      {repeat && passphrase !== repeat && <p className='ob-page-intro'>The passphrases do not match.</p>}
      <FormError message={error} />
      <div className='ob-form-actions'>
        <button className='ob-primary' disabled={busy || short || passphrase !== repeat}>
          {busy ? 'Setting passphrase…' : 'Set passphrase'}
        </button>
      </div>
    </form>
  )
}

export function UnlockForm({
  status,
  onStatus,
  reason
}: {
  status: LockStatus
  onStatus: (s: LockStatus) => void
  reason?: string
}) {
  const [passphrase, setPassphrase] = useState('')
  const { busy, error, run } = useSubmit()
  return (
    <form
      className='ob-wallet-lock'
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        void run(async () => {
          const next = await unlockWallet(passphrase)
          setPassphrase('')
          onStatus(next)
        }, 'The wallet could not be unlocked.')
      }}
    >
      <h2>Wallet locked</h2>
      <p>{reason ?? 'Unlock the wallet to pay or sign. Balances and receive addresses work while it is locked.'}</p>
      <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} autoFocus />
      {status.retryAfterSecs > 0 && (
        <p className='ob-page-intro'>Too many wrong tries. Wait {status.retryAfterSecs} seconds.</p>
      )}
      <FormError message={error} />
      <div className='ob-form-actions'>
        {status.rememberDevice && !servedByHomeNode() && nodeTarget() !== "home" && (
          <button
            className='ob-secondary'
            type='button'
            disabled={busy}
            onClick={() => void run(async () => onStatus(await unlockRemembered()), 'This device could not unlock the wallet.')}
          >
            Unlock with this device
          </button>
        )}
        <button className='ob-primary' disabled={busy || !passphrase}>
          {busy ? 'Unlocking…' : 'Unlock wallet'}
        </button>
      </div>
    </form>
  )
}

function BackupReminder({ onStatus }: { onStatus: (s: LockStatus) => void }) {
  const [passphrase, setPassphrase] = useState('')
  const [shown, setShown] = useState<{ phrase: string[]; confirmWords: number[] } | null>(null)
  const { busy, error, run } = useSubmit()
  const [localOnly, setLocalOnly] = useState(false)
  if (localOnly) return <div className='ob-wallet-lock'><h2>Backup recovery is local only</h2><p>Open this node on its own computer to view the backup phrase. Remote access cannot reveal a confirmed backup.</p></div>
  if (shown)
    return (
      <BackupCheck
        phrase={shown.phrase}
        positions={shown.confirmWords}
        onDone={next => {
          setShown(null)
          if (next) onStatus(next)
        }}
      />
    )
  return (
    <form
      className='ob-wallet-lock'
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        void run(async () => {
          try {
            setShown(await revealPhrase(passphrase))
          } catch (err) {
            if (err instanceof Error && err.message.includes("backup phrase is shown only on the node's own machine")) {
              setLocalOnly(true)
              void fetchLock().then(onStatus, () => undefined)
              return
            }
            throw err
          } finally { setPassphrase('') }
        }, 'The backup phrase could not be shown.')
      }}
    >
      <h2>Back up this wallet</h2>
      <p>The backup phrase has not been confirmed. Enter your passphrase to see it again.</p>
      <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} />
      <FormError message={error} />
      <div className='ob-form-actions'>
        <button className='ob-secondary' disabled={busy || !passphrase}>
          Show backup phrase
        </button>
      </div>
    </form>
  )
}

function LockSettings({ status, onStatus }: { status: LockStatus; onStatus: (s: LockStatus) => void }) {
  const [minutes, setMinutes] = useState(status.autoLockMinutes),
    [remember, setRemember] = useState(status.rememberDevice),
    [passphrase, setPassphrase] = useState('')
  const { busy, error, run } = useSubmit()
  const [min, max] = status.autoLockRange
  const remote = servedByHomeNode() || nodeTarget() === "home"
  const choices = [...new Set([...AUTO_LOCK_CHOICES, status.autoLockMinutes])]
    .filter(m => m >= min && m <= max)
    .sort((a, b) => a - b)
  const changed = minutes !== status.autoLockMinutes || (!remote && remember !== status.rememberDevice)
  return (
    <form
      className='ob-wallet-lock'
      onSubmit={(e: FormEvent) => {
        e.preventDefault()
        if (!changed || !passphrase) return
        void run(async () => {
          onStatus(await updateLockSettings({ passphrase, autoLockMinutes: minutes, ...(!remote ? { rememberDevice: remember } : {}) }))
          setPassphrase('')
        }, 'The lock settings could not be saved.')
      }}
    >
      <label className='ob-field'>
        <span>Lock after inactivity</span>
        <select value={minutes} onChange={e => setMinutes(Number(e.target.value))}>
          {choices.map(m => (
            <option key={m} value={m}>
              {m === 1 ? '1 minute' : `${m} minutes`}
            </option>
          ))}
        </select>
      </label>
      {status.rememberAvailable && !servedByHomeNode() && nodeTarget() !== "home" && (
        <label className='ob-checkbox'>
          <input type='checkbox' checked={remember} onChange={e => setRemember(e.target.checked)} /> Remember on this
          device. Viewing stays open. Payments still ask for the passphrase.
        </label>
      )}
      {changed && <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} />}
      <FormError message={error} />
      {changed && passphrase ? (
        <div className='ob-form-actions'>
          <button className='ob-secondary' disabled={busy}>
            {busy ? 'Saving…' : 'Save lock settings'}
          </button>
        </div>
      ) : null}
    </form>
  )
}

/** The lock card on the wallet page. Settings stay collapsed until opened. */
export function WalletLockPanel({ lock, purpose = "wallet" }: { lock: LockResource; purpose?: "wallet" | "moderator" }) {
  const status = lock.data
  const [error, setError] = useState('')
  if (!status) {
    if (lock.error && shopPairingMessage(lock.error)) return null
    return <p role='status'>{lock.error || 'Checking the wallet lock…'}</p>
  }
  if (status.state === 'setup') return <SetupWallet status={status} onStatus={lock.update} />
  if (status.state === 'locked') return <UnlockForm status={status} onStatus={lock.update} reason={purpose === "moderator" ? "Unlock this node’s wallet to sign a final escrow ruling. You can review disputes while it stays locked." : undefined} />
  const minutesLeft = status.lockInSecs == null ? null : Math.max(1, Math.ceil(status.lockInSecs / 60))
  return (
    <section className='ob-wallet-fund' aria-labelledby='ob-wallet-lock'>
      <div className='ob-section-heading ob-lock-line'>
        <h2 id='ob-wallet-lock'>{purpose === 'moderator' ? 'Wallet unlocked · signing enabled' : 'Viewing unlocked · payments require passphrase'}</h2>
        <button
          className='ob-secondary'
          type='button'
          onClick={() => {
            setError('')
            void lockWallet().then(lock.update, (e: unknown) =>
              setError(e instanceof Error ? e.message : 'The wallet could not be locked.')
            )
          }}
        >
          Lock now
        </button>
      </div>
      <FormError message={error} />
      {!status.backupConfirmed && <BackupReminder onStatus={lock.update} />}
      <details className='ob-lock-details'>
        <summary>Lock settings</summary>
        <p>
          {minutesLeft == null
            ? 'The wallet locks after a period without activity.'
            : `The wallet locks after ${minutesLeft} more ${minutesLeft === 1 ? 'minute' : 'minutes'} without activity.`}
        </p>
        <LockSettings key={`${status.autoLockMinutes}-${status.rememberDevice}`} status={status} onStatus={lock.update} />
      </details>
    </section>
  )
}

export function Dialog({ label, onCancel, children, className = '' }: { label: string; onCancel: () => void; children: ReactNode; className?: string }) {
  const dialog = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    const current = dialog.current
    current?.showModal()
    current?.querySelector<HTMLInputElement>('[data-dialog-autofocus]')?.focus({ preventScroll: true })
    return () => {
      current?.close()
      previous?.focus()
    }
  }, [])
  return (
    <dialog
      ref={dialog}
      className={`ob-pay-confirm ${className}`}
      aria-label={label}
      onCancel={e => {
        e.preventDefault()
        onCancel()
      }}
    >
      {children}
    </dialog>
  )
}

/** Unlock prompt for a payment started while the wallet is locked. */
export function UnlockDialog({
  status,
  onCancel,
  onUnlocked,
  reason = 'Unlock the wallet to make this payment.'
}: {
  status: LockStatus
  onCancel: () => void
  onUnlocked: (s: LockStatus) => void
  reason?: string
}) {
  return (
    <Dialog label='Unlock wallet' onCancel={onCancel}>
      {status.state === 'setup' ? (
        <p>Set a wallet passphrase on the wallet page before paying.</p>
      ) : (
        <UnlockForm status={status} onStatus={onUnlocked} reason={reason} />
      )}
      <div className='ob-form-actions'>
        <button className='ob-secondary' onClick={onCancel}>
          Cancel
        </button>
      </div>
    </Dialog>
  )
}

/**
 * Step-up for one payment: the node's quoted terms, exactly as the node will
 * enforce them, and the passphrase. Resolves to a one-time spend ticket.
 */
export function SpendConfirmDialog({
  quote,
  seller,
  onCancel,
  onTicket
}: {
  quote: Quote
  seller: string
  onCancel: () => void
  onTicket: (ticket: Ticket) => void
}) {
  const [passphrase, setPassphrase] = useState('')
  const { busy, error, run } = useSubmit()
  const mainnet = quote.network === 'mainnet'
  return (
    <Dialog label={mainnet ? 'Send real bitcoin?' : 'Confirm payment'} onCancel={onCancel}>
      <form className='ob-spend-confirm'
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          void run(async () => onTicket(await issueTicket(quote.quote, passphrase)), 'The payment could not be confirmed.')
        }}
      >
        <h2>{mainnet ? 'Send real bitcoin?' : 'Confirm payment'}</h2>
        <dl className='ob-spend-terms'>
          <dt>Pay</dt>
          <dd>
            <strong>
              {quote.amountSats.toLocaleString()} sats ({bitcoinAmount(quote.amountSats)} BTC)
            </strong>{' '}
            to {seller}
          </dd>
          <dt>Network fee</dt>
          <dd>
            At most {quote.feeCapSats.toLocaleString()} sats ({quote.feeRateSatVb} sat/vB now)
          </dd>
          <dt>Address</dt>
          <dd>
            <CopyAddress value={quote.address} label='Copy payment address'/>
          </dd>
          <dt>Network</dt>
          <dd>{mainnet ? 'Mainnet' : 'Testnet4 · test coins, no value'}</dd>
          <dt>Order</dt>
          <dd>
            <code>{quote.orderId}</code>
          </dd>
        </dl>
        <p className='ob-page-intro'>
          Your node will refuse to send anything else. Bitcoin payments cannot be reversed. This confirmation expires in{' '}
          {quote.expiresInSecs} seconds.
        </p>
        <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} autoFocus />
        <FormError message={error} />
        <div className='ob-form-actions'>
          <button className='ob-secondary' type='button' onClick={onCancel}>
            Cancel
          </button>
          <button className='ob-primary' disabled={busy || !passphrase}>
            {busy ? 'Confirming…' : mainnet ? 'Confirm mainnet payment' : 'Confirm payment'}
          </button>
        </div>
      </form>
    </Dialog>
  )
}
