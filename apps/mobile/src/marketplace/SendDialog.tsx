import { Fragment, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { isLocked, type LockStatus } from '../walletLock'
import { heldBack, quoteExpired, sendError } from '../walletSend'
import { EXTERNAL_TX_NOTE } from '../nodeWallet'
import { CopyPaymentValue } from './NodePayments'
import { RailMark, type Rail } from './RailMark'
import { Dialog, FormError, Passphrase, UnlockForm, type LockResource } from './WalletLock'

/** What the user typed on the first screen. */
export type SendInput = { destination: string; amount: string; max: boolean; linkCoins: boolean }
export type SendDone = { idLabel: string; id: string; summary: string; note?: string }

/** One wallet's send: how it checks, quotes, shows, and sends. The node enforces every term. */
export type SendRail<Q> = {
  rail: Rail
  title: string
  /** The network, with the honest test-coin label where it applies. */
  network: string
  destination: { label: string; placeholder: string; multiline?: boolean; help?: string }
  /** Omitted when the destination carries the amount (a Lightning invoice). */
  amount?: { label: string; unit: string; max: boolean }
  /** Bitcoin only: change linked to earlier payments, which the node leaves out unless asked. */
  linkable?: boolean
  /** A problem the app can see before asking the node, or `null`. */
  check(input: SendInput): string | null
  quote(input: SendInput): Promise<Q>
  review(quote: Q): { rows: [string, ReactNode][]; note: string; heldBackSats?: number; expiresInSecs: number }
  send(quote: Q, passphrase: string): Promise<SendDone>
  sendLabel: string
}

type Step<Q> = { kind: 'form' } | { kind: 'unlock' } | { kind: 'review'; quote: Q } | { kind: 'done'; done: SendDone }

export function SendDialog<Q>({ config, lock, onCancel, onSent }: { config: SendRail<Q>; lock: LockResource; onCancel: () => void; onSent: () => void }) {
  const [input, setInput] = useState<SendInput>({ destination: '', amount: '', max: false, linkCoins: false })
  const [step, setStep] = useState<Step<Q>>({ kind: 'form' })
  const [passphrase, setPassphrase] = useState('')
  const [error, setError] = useState('')
  const [expired, setExpired] = useState(false)
  const [busy, setBusy] = useState(false)
  const working = useRef(false)

  async function guarded(task: () => Promise<void>) {
    if (working.current) return
    working.current = true
    setBusy(true)
    setError('')
    try { await task() }
    finally { working.current = false; setBusy(false) }
  }
  function review(next: SendInput = input) {
    const problem = config.check(next)
    if (problem) return setError(problem)
    void guarded(async () => {
      try {
        setExpired(false)
        setStep({ kind: 'review', quote: await config.quote(next) })
      } catch (e) {
        if (isLocked(e)) { await lock.refresh(); setStep({ kind: 'unlock' }); return }
        setStep({ kind: 'form' })
        setError(sendError(e, 'The send could not be reviewed.'))
      }
    })
  }
  function send(quote: Q) {
    void guarded(async () => {
      try {
        const done = await config.send(quote, passphrase)
        setPassphrase('')
        setStep({ kind: 'done', done })
        onSent()
      } catch (e) {
        if (isLocked(e)) { await lock.refresh(); setStep({ kind: 'unlock' }); return }
        if (quoteExpired(e)) setExpired(true)
        setError(sendError(e, 'The send failed.'))
      }
    })
  }
  function unlocked(status: LockStatus) {
    lock.update(status)
    setStep({ kind: 'form' })
    if (status.state === 'unlocked') review()
  }
  function linkAndReview() {
    const next = { ...input, linkCoins: true }
    setInput(next)
    review(next)
  }
  const field = (key: keyof SendInput) => (value: string) => setInput(current => ({ ...current, [key]: value }))

  const heading = <><h2><RailMark rail={config.rail}/>{config.title}</h2><p className='ob-send-network'>{config.network}</p></>
  return <Dialog label={config.title} onCancel={busy ? () => undefined : onCancel} className='ob-send-dialog'>
    {step.kind === 'unlock' && lock.data && <>
      {lock.data.state === 'setup'
        ? <p>Set a wallet passphrase in wallet settings before sending.</p>
        : <UnlockForm status={lock.data} onStatus={unlocked} reason='Unlock the wallet to review this send. Sending still asks for the passphrase.'/>}
      <div className='ob-form-actions'><button type='button' className='ob-secondary' onClick={() => setStep({ kind: 'form' })}>Back</button></div>
    </>}

    {step.kind === 'form' && <form className='ob-send-form' onSubmit={(e: FormEvent) => { e.preventDefault(); review() }}>
      {heading}
      <label className='ob-field'>
        <span>{config.destination.label}</span>
        {config.destination.multiline
          ? <textarea value={input.destination} onChange={e => field('destination')(e.target.value)} placeholder={config.destination.placeholder} rows={4} spellCheck={false} autoComplete='off' data-dialog-autofocus/>
          : <input value={input.destination} onChange={e => field('destination')(e.target.value)} placeholder={config.destination.placeholder} spellCheck={false} autoComplete='off' autoCapitalize='off' data-dialog-autofocus/>}
      </label>
      {config.destination.help && <p className='ob-page-intro'>{config.destination.help}</p>}
      {config.amount && <div className='ob-send-amount'>
        <label className='ob-field'>
          <span>{config.amount.label}</span>
          <input inputMode='decimal' value={input.max ? '' : input.amount} disabled={input.max} placeholder={input.max ? 'Everything you can spend' : config.amount.unit} onChange={e => field('amount')(e.target.value)} autoComplete='off'/>
        </label>
        {config.amount.max && <label className='ob-checkbox'><input type='checkbox' checked={input.max} onChange={e => setInput(current => ({ ...current, max: e.target.checked }))}/> Send max</label>}
      </div>}
      {config.linkable && input.linkCoins && <p className='ob-page-intro'>Change from earlier payments is included. Spending it links those payments to this send on chain.</p>}
      <FormError message={error}/>
      {config.linkable && !input.linkCoins && heldBack(error) && <button type='button' className='ob-text-link' onClick={linkAndReview}>Include that change and review again</button>}
      <div className='ob-form-actions'>
        <button type='button' className='ob-secondary' onClick={onCancel}>Cancel</button>
        <button className='ob-primary' disabled={busy || !input.destination.trim()}>{busy ? 'Checking…' : 'Review'}</button>
      </div>
    </form>}

    {step.kind === 'review' && (() => {
      const shown = config.review(step.quote)
      return <form className='ob-spend-confirm' onSubmit={(e: FormEvent) => { e.preventDefault(); send(step.quote) }}>
        {heading}
        <dl className='ob-spend-terms'>{shown.rows.map(([label, value]) => <Fragment key={label}><dt>{label}</dt><dd>{value}</dd></Fragment>)}</dl>
        {config.linkable && !!shown.heldBackSats && <p className='ob-page-intro'>{shown.heldBackSats.toLocaleString('en-US')} sats of change from earlier payments was left out so those payments stay unlinked. <button type='button' className='ob-text-link' onClick={linkAndReview}>Include it</button></p>}
        <p className='ob-page-intro'>{shown.note} This review expires in {shown.expiresInSecs} seconds.</p>
        <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} autoFocus/>
        <FormError message={error}/>
        <div className='ob-form-actions'>
          <button type='button' className='ob-secondary' disabled={busy} onClick={() => { setError(''); setStep({ kind: 'form' }) }}>Back</button>
          {expired
            ? <button type='button' className='ob-primary' disabled={busy} onClick={() => review()}>{busy ? 'Checking…' : 'Review again'}</button>
            : <button className='ob-primary' disabled={busy || !passphrase}>{busy ? 'Sending…' : config.sendLabel}</button>}
        </div>
      </form>
    })()}

    {step.kind === 'done' && <div className='ob-send-done' role='status'>
      {heading}
      <p><strong>{step.done.summary}</strong></p>
      <CopyPaymentValue label={step.done.idLabel} value={step.done.id} help={EXTERNAL_TX_NOTE}/>
      {step.done.note && <p className='ob-page-intro'>{step.done.note}</p>}
      <div className='ob-form-actions'><button type='button' className='ob-primary' onClick={onCancel}>Done</button></div>
    </div>}
  </Dialog>
}
