import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode
} from 'react'
import QRCode from 'qrcode'
import {
  fetchOrder,
  fetchOrders,
  type BuyerOrder,
  type CurrencyCode,
  type RateTable,
  type SellerOrder
} from '../catalog'
import {
  amountDue,
  bitcoinAmount,
  bitcoinUri,
  fetchNetwork,
  fetchWallet,
  newWalletAddress,
  payOrder,
  privacyStatus,
  EXTERNAL_TX_NOTE,
  type NetworkInfo,
  type WalletActivity
} from '../nodeWallet'
import { chainHeightLabel, networkInfo } from '../networkInfo'
import { connectionRows } from '../walletConnection'
import { paymentStatusView } from '../paymentStatus'
import {
  canOfferBitcoinPay,
  confirmationTarget,
  directPayWarning,
  sellerZcashNote,
  moneroDue,
  zcashDue,
  ORDER_STATUS,
  orderPollingDone,
  payKind
} from '../orderState'
import { useNodeResource } from '../useNodeResource'
import { isLocked, lockWallet, quotePayment, type LockStatus, type Quote, type Ticket } from '../walletLock'
import { RailMark } from './RailMark'
import { Dialog, SpendConfirmDialog, UnlockDialog, WalletLockPanel, useLockState } from './WalletLock'
import { PageHeading } from './PageHeading'
import { onThisComputer, pairShop, shopPairingMessage } from '../catalog'
import { pairingKind, pairingWalletPrompt } from '../pairingUi'
import { SpeedUpPayment } from './SpeedUpPayment'

function usePaymentState(activeWallet: boolean, onWallet: () => void) {
  const network = useNodeResource(fetchNetwork, 30000)
  const wallet = useNodeResource(fetchWallet, 15000, activeWallet)
  const lock = useLockState()
  return { network, wallet, lock, onWallet }
}
type PaymentState = ReturnType<typeof usePaymentState>
const NodeContext = createContext<PaymentState | null>(null)
export function usePairedNode() {
  const node = useContext(NodeContext)
  if (!node) throw new Error('Missing paired node context')
  return node
}
export function NodePaymentsProvider({
  activeWallet,
  onWallet,
  children
}: {
  activeWallet: boolean
  onWallet: () => void
  children: ReactNode
}) {
  return (
    <NodeContext.Provider value={usePaymentState(activeWallet, onWallet)}>
      {children}
    </NodeContext.Provider>
  )
}
export function NetworkBadge() {
  const { network } = usePairedNode()
  const info = network.data ? networkInfo(network.data) : null
  const warning = network.data ? privacyStatus(network.data) : ''
  const networkLabel = info
    ? info.network === 'testnet4'
      ? 'Testnet4 · test coins, no value'
      : 'Mainnet'
    : ''
  return (
    <div
      className={`ob-network-badge ${network.error ? 'is-offline' : !network.data ? 'is-loading' : ''}`}
      role='status'
    >
      <span className='ob-status-dot' aria-hidden='true' />
      <span className='ob-network-facts'>
        {network.error
          ? 'Network unavailable · payments disabled'
          : !info
            ? 'Checking bitcoin network…'
            : (
              <>
                <span>{networkLabel}</span>
                <span>{` · ${chainHeightLabel(info)}`}</span>
              </>
            )}
      </span>
      {warning ? <span className='ob-network-warning'>{warning}</span> : null}
    </div>
  )
}
function ConnectionCard({ info }: { info: NetworkInfo }) {
  const rows = connectionRows(info)
  return (
    <details className='ob-wallet-connection'>
      <summary>Connection and privacy<span>{privacyStatus(info) || 'Connection details'}</span></summary>
      <dl className='ob-privacy-rows'>
        {rows.map(row => (
          <div key={row.label}>
            <dt>{row.label}</dt>
            <dd>{row.value}</dd>
          </div>
        ))}
      </dl>
    </details>
  )
}
export function WalletAmount({
  sats,
  rates,
  currency,
  showFiat = true
}: {
  sats: number
  rates: RateTable
  currency: CurrencyCode
  showFiat?: boolean
}) {
  const rate = showFiat ? rates[currency] : undefined
  return (
    <div className='ob-wallet-amount'>
      <strong>
        {sats.toLocaleString('en-US')} <small>sats</small>
      </strong>
      <span>
        {bitcoinAmount(sats)} BTC
        {rate
          ? ` ≈ ${new Intl.NumberFormat('en-US', { style: 'currency', currency }).format((sats / 1e8) * rate)}`
          : showFiat
            ? ' · fiat estimate unavailable'
            : ''}
      </span>
    </div>
  )
}
export function CopyPaymentValue({
  label,
  value,
  children,
  button = false
}: {
  label: string
  value: string
  children?: ReactNode
  button?: boolean
}) {
  const [message, setMessage] = useState('')
  const timer = useRef<number | null>(null)
  const copyLabel = `Copy ${label.charAt(0).toLowerCase()}${label.slice(1)}`
  useEffect(() => {
    setMessage('')
    return () => { if (timer.current) window.clearTimeout(timer.current) }
  }, [value])
  return (
    <div className='ob-copy-value'>
      <span>{label}</span>
      <code>{value}</code>
      <div className='ob-copy-actions'>
        <button
          className={button ? 'ob-secondary' : 'ob-text-link'}
          type='button'
          aria-live='polite'
          onClick={() => {
            void navigator.clipboard.writeText(value).then(
              () => {
                setMessage('Copied')
                if (timer.current) window.clearTimeout(timer.current)
                timer.current = window.setTimeout(() => setMessage(''), 2000)
              },
              () => setMessage('Copy the text above manually')
            )
          }}
        >
          {message || copyLabel}
        </button>
        {children}
      </div>
    </div>
  )
}
export function PaymentQr({ value, paper = false }: { value: string; paper?: boolean }) {
  const [src, setSrc] = useState('')
  const [error, setError] = useState(false)
  useEffect(() => {
    let active = true
    setSrc('')
    setError(false)
    void QRCode.toDataURL(value, {
      width: paper ? 448 : 224,
      margin: paper ? 2 : 4,
      errorCorrectionLevel: 'M',
      color: { dark: '#071824', light: paper ? '#fbfaf7' : '#ffffff' }
    }).then(
      s => {
        if (active) setSrc(s)
      },
      () => {
        if (active) setError(true)
      }
    )
    return () => {
      active = false
    }
  }, [value, paper])
  return error ? (
    <p role='alert'>QR code unavailable. Copy the address instead.</p>
  ) : src ? (
    <img
      className={paper ? 'ob-payment-qr is-paper' : 'ob-payment-qr'}
      width={224}
      height={224}
      src={src}
      alt='Bitcoin payment QR code'
      data-payment-uri={value}
    />
  ) : (
    <p role='status'>Preparing QR code…</p>
  )
}
export function WalletPanel({ rates, currency, onOrders }: {
  rates: RateTable
  currency: CurrencyCode
  onOrders?: () => void
}) {
  const { wallet, network, lock } = usePairedNode()
  const orders = useNodeResource(fetchOrders, 15000, !!onOrders)
  const [busy, setBusy] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState('')
  const [settings, setSettings] = useState(false)
  const [receive, setReceive] = useState(false)
  const [locking, setLocking] = useState(false)
  const [freshAddress, setFreshAddress] = useState<{ network: string; address: string } | null>(null)
  const current = wallet.data
  const status = lock.data
  const mismatch = !!current && !!network.data && current.network !== network.data.network
  const pairing = [error, wallet.error, network.error, lock.error].some(message => !!message && shopPairingMessage(message))
  const pairingPrompt = pairingWalletPrompt(pairingKind(onThisComputer()))
  const address = current && freshAddress?.network === current.network ? freshAddress.address : current?.receiveAddress
  const showFiat = current?.network === 'mainnet'
  const receiveBlocked = !address || mismatch || !!wallet.error || !!network.error || pairing
  const dueOrders = (orders.data ?? []).filter(order =>
    order.status === ORDER_STATUS.awaitingPayment && payKind(order) === 'btc' && !order.payTxid && !order.paying && amountDue(order.totalSats, order.receivedSats) > 0)
  async function refresh() {
    if (refreshing) return
    setRefreshing(true)
    try { await Promise.all([wallet.refresh(), network.refresh(), lock.refresh(), orders.refresh()]) }
    finally { setRefreshing(false) }
  }
  async function nextAddress() {
    if (busy || receiveBlocked) return
    setBusy(true)
    setError('')
    try {
      setFreshAddress(await newWalletAddress())
      await wallet.refresh()
    } catch (e) { setError(e instanceof Error ? e.message : 'A new address could not be created.') }
    finally { setBusy(false) }
  }
  async function enterPairingCode() {
    setError('')
    try { await pairShop(); await refresh() }
    catch (e) {
      if (!(e instanceof Error && shopPairingMessage(e.message)))
        setError(e instanceof Error ? e.message : 'The shop could not be paired.')
    }
  }
  async function lockNow() {
    if (locking) return
    setLocking(true)
    setError('')
    try { lock.update(await lockWallet()) }
    catch (e) { setError(e instanceof Error ? e.message : 'The wallet could not be locked.') }
    finally { setLocking(false) }
  }
  const stale = !!wallet.error || !!current?.syncError || mismatch || !current?.lastSync
  return (
    <section className='ob-wallet-page ob-wallet-overview'>
      <PageHeading title="Your wallet" description="Pay for purchases. Store proceeds are held separately." action={
        <button type='button' className='ob-secondary' aria-label='Wallet settings' onClick={() => setSettings(true)}>Settings</button>
      } />
      {pairing ? <div className='ob-wallet-alert' role='status'><p>{pairingPrompt.text}</p><button className='ob-secondary' onClick={() => void enterPairingCode()}>{pairingPrompt.action}</button></div>
        : (error || wallet.error || network.error || lock.error) && <p className='ob-form-error' role='alert'>{error || wallet.error || network.error || lock.error}{current && wallet.error ? ' Showing the last available balance.' : ''}</p>}
      {mismatch && <p className='ob-form-error' role='alert'>Wallet and node networks do not match. Payments are disabled.</p>}
      <section className='ob-wallet-summary' aria-label='Wallet balance'>
        <div className='ob-wallet-toolbar'>
          <span className='ob-wallet-account'><RailMark rail='bitcoin' /> Bitcoin · {current?.network === 'testnet4' ? 'Testnet4' : current?.network === 'mainnet' ? 'Mainnet' : 'Node wallet'}</span>
          <div className='ob-wallet-security'>
            <span>{!status ? (lock.error ? 'Lock state unavailable' : 'Checking lock…') : status.state === 'unlocked' ? 'Unlocked · payments require passphrase' : status.state === 'locked' ? 'Locked · receiving available' : 'Wallet setup required'}</span>
            {status?.state === 'unlocked' ? <button type='button' className='ob-text-link' disabled={locking} onClick={() => void lockNow()}>{locking ? 'Locking…' : 'Lock now'}</button>
              : status && <button type='button' className='ob-text-link' onClick={() => setSettings(true)}>{status.state === 'locked' ? 'Unlock wallet' : 'Set up wallet'}</button>}
          </div>
        </div>
        <div className='ob-wallet-balance-grid'>
          <article className='ob-wallet-spendable'>
            <h2>{stale && current ? 'Last known spendable balance' : 'Spendable balance'}</h2>
            {current ? <WalletAmount sats={current.balance.spendableSats} rates={rates} currency={currency} showFiat={showFiat} />
              : <p className='ob-wallet-loading' role='status'>{wallet.loading ? 'Loading balance…' : 'Balance unavailable'}</p>}
            {current?.network === 'testnet4' && <p className='ob-wallet-test-note'>Test coins · no monetary value</p>}
          </article>
          <article className='ob-wallet-pending'>
            <h2>Pending confirmation</h2>
            {current ? <WalletAmount sats={current.balance.pendingSats} rates={rates} currency={currency} showFiat={showFiat} /> : <span>—</span>}
            <p>Available to spend after confirmation.</p>
          </article>
        </div>
        <div className='ob-wallet-summary-footer'>
          <div className='ob-wallet-actions'>
            <button type='button' className='ob-primary' disabled={receiveBlocked} onClick={() => setReceive(true)}>Receive bitcoin</button>
            {onOrders && <button type='button' className='ob-secondary' onClick={onOrders}>Your orders{dueOrders.length ? ` · ${dueOrders.length} unpaid` : ''}</button>}
          </div>
          <div className='ob-wallet-sync'>
            <span className={`ob-status-dot ${stale ? 'is-loading' : ''}`} aria-hidden='true' />
            <span>{current?.lastSync ? `Last synced ${new Date(current.lastSync * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}` : 'Not synced yet'}</span>
            <button type='button' className='ob-text-link' disabled={refreshing || wallet.loading} onClick={() => void refresh()}>{refreshing ? 'Refreshing…' : 'Refresh wallet'}</button>
          </div>
        </div>
        {current?.syncError && <p className='ob-wallet-sync-error' role='alert'>Balance may be out of date. {current.syncError}</p>}
      </section>
      {status && (status.state === 'setup' || !status.backupConfirmed) && <div className='ob-wallet-alert' role='status'>
        <p>{status.state === 'setup' ? 'Protect your wallet before making a payment.' : 'Your wallet backup has not been confirmed.'}</p>
        <button type='button' className='ob-secondary' onClick={() => setSettings(true)}>{status.state === 'setup' ? 'Protect wallet' : 'Back up wallet'}</button>
      </div>}
      {dueOrders.length > 0 && onOrders && <div className='ob-wallet-attention'>
        <div><strong>{dueOrders.length} {dueOrders.length === 1 ? 'order awaits' : 'orders await'} payment</strong><p>Review and pay each seller separately from your orders.</p></div>
        <button type='button' className='ob-text-link' onClick={onOrders}>Review orders</button>
      </div>}
      <div className='ob-wallet-workspace'>
        <WalletActivityList activity={current?.activity ?? []} available={!!current} stale={!!wallet.error || !!current?.syncError} />
        <aside className='ob-wallet-receive-panel' aria-labelledby='ob-receive-title'>
          <h2 id='ob-receive-title'>Fund your wallet</h2>
          <p>{current?.network === 'testnet4' ? 'Receive testnet4 bitcoin from a faucet or another testnet4 wallet.' : 'Transfer bitcoin here from another wallet to pay for purchases.'}</p>
          {address && !mismatch && !pairing ? <div className='ob-wallet-receive'>
            <CopyPaymentValue label='Receive address' value={address} button />
            <div className='ob-wallet-receive-actions'>
              <button className='ob-secondary' type='button' disabled={receiveBlocked} onClick={() => setReceive(true)}>Show QR code</button>
              <button className='ob-text-link' type='button' disabled={busy || receiveBlocked} onClick={() => void nextAddress()}>{busy ? 'Creating address…' : 'New address'}</button>
            </div>
          </div> : <p className='ob-wallet-note'>The receive address is unavailable until your node connects.</p>}
          {current?.network === 'testnet4' && <p className='ob-wallet-faucet'>Need test coins? <a className='ob-inline-link' href='https://mempool.space/testnet4/faucet' target='_blank' rel='noreferrer'>Open testnet4 faucet ↗</a><small>Opens an external website.</small></p>}
        </aside>
      </div>
      {network.data && <ConnectionCard info={network.data} />}
      {settings && <Dialog label='Wallet settings' onCancel={() => setSettings(false)}><h2>Wallet settings</h2><WalletLockPanel lock={lock} /><div className='ob-form-actions'><button type='button' className='ob-secondary' onClick={() => setSettings(false)}>Done</button></div></Dialog>}
      {receive && !receiveBlocked && address && <Dialog label='Receive bitcoin' onCancel={() => setReceive(false)}>
        <div className='ob-wallet-receive-dialog'><h2>Receive bitcoin</h2><p>{current?.network === 'testnet4' ? 'Testnet4 only · test coins, no value' : 'Bitcoin mainnet only'}</p><PaymentQr value={bitcoinUri(address)} paper /><p className='ob-wallet-dialog-address'>{address}</p><p>Scan this code from the wallet sending your funds.</p></div>
        <div className='ob-form-actions'><button className='ob-secondary' type='button' onClick={() => setReceive(false)}>Done</button></div>
      </Dialog>}
    </section>
  )
}
function WalletActivityList({ activity, available, stale }: { activity: WalletActivity[]; available: boolean; stale: boolean }) {
  const [filter, setFilter] = useState('all')
  const visible = activity.filter(tx => filter === 'all' || tx.direction === filter)
  return <section className='ob-wallet-ledger' aria-labelledby='ob-wallet-activity'>
    <header><h2 id='ob-wallet-activity'>Activity</h2><label>Show<select aria-label='Filter wallet activity' value={filter} onChange={event => setFilter(event.target.value)}><option value='all'>All activity</option><option value='in'>Received</option><option value='out'>Sent</option></select></label></header>
    {!available ? <p className='ob-wallet-empty'>Activity is unavailable until your node connects.</p>
      : visible.length === 0 ? <p className='ob-wallet-empty'>{activity.length ? 'No activity matches this filter.' : stale ? 'No activity in the last available wallet data.' : 'No transactions yet. Fund your wallet to get started.'}</p>
      : <ul className='ob-wallet-activity'>{visible.map(tx => <li key={tx.txid}>
        <details><summary><span className={`ob-wallet-direction ${tx.direction === 'in' ? 'is-incoming' : ''}`} aria-hidden='true'>{tx.direction === 'in' ? '↙' : '↗'}</span><span className='ob-wallet-transaction'><strong>{tx.label}</strong><span className='ob-activity-meta'>{tx.confirmed ? 'Confirmed' : 'Pending'}{tx.time ? ` · ${new Date(tx.time * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}` : ''}</span></span><span className='ob-activity-amount'>{tx.direction === 'in' ? '+' : '−'}{tx.sats.toLocaleString('en-US')} sats</span><span className='ob-wallet-expand' aria-hidden='true'>⌄</span></summary>
          <div className='ob-wallet-transaction-details'><CopyPaymentValue label='Transaction ID' value={tx.txid} />{tx.direction === 'out' && <p>Net wallet movement, including network fees.</p>}<p>{EXTERNAL_TX_NOTE}</p></div>
        </details>
      </li>)}</ul>}
  </section>
}
export function OrderPayment({
  id,
  initial
}: {
  id: string
  initial: BuyerOrder | SellerOrder
}) {
  const { network, wallet, lock, onWallet } = usePairedNode()
  const [error, setError] = useState(''),
    [sending, setSending] = useState(false),
    [step, setStep] = useState<
      { kind: 'unlock' } | { kind: 'confirm'; quote: Quote } | null
    >(null)
  const submitting = useRef(false)
  const resource = useNodeResource(
    () => fetchOrder(id),
    12000,
    true,
    orderPollingDone
  )
  const order = resource.data ?? initial
  const due = amountDue(order.totalSats, order.receivedSats)
  const kind = payKind(order)
  const monero = moneroDue(order.payment)
  const zcash = zcashDue(order.payment)
  const escrow = kind === 'escrow'
  const offerBitcoinPay = canOfferBitcoinPay(order, kind, due)
  const directWarning = directPayWarning(order, kind)
  const net = network.data?.network
  const orderNetwork = order.network ?? order.payment?.network
  const mismatch =
    kind === 'btc' &&
    !!net &&
    ((!!orderNetwork && orderNetwork !== net) ||
      (!!wallet.data && wallet.data.network !== net))
  const view = paymentStatusView({
    network: orderNetwork ?? net ?? 'testnet4',
    status: order.status,
    confirmations: order.confirmations,
    requiredConfirmations: confirmationTarget(
      kind,
      (monero ?? zcash)?.confirmationsNeeded,
      network.data?.confirmations
    ),
    dueSats: due,
    sending: order.paying || sending,
    payTxid: order.payTxid,
    wallet: kind === 'btc' ? wallet.data?.balance : null
  })
  const lowBalance = !!view.balance
  const blocked =
    !offerBitcoinPay ||
    !net ||
    !orderNetwork ||
    mismatch ||
    !!network.error ||
    !!wallet.error ||
    !wallet.data ||
    wallet.loading ||
    !!resource.error ||
    resource.loading ||
    !!wallet.data.syncError ||
    lowBalance ||
    sending ||
    order.paying
  // Unlock if needed, then ask the node for the exact terms and step up.
  async function start(status = lock.data) {
    if (submitting.current || blocked || !net) return
    setError('')
    if (status?.state !== 'unlocked') {
      setStep({ kind: 'unlock' })
      return
    }
    submitting.current = true
    try {
      setStep({ kind: 'confirm', quote: await quotePayment(id) })
    } catch (e) {
      if (isLocked(e)) {
        await lock.refresh()
        setStep({ kind: 'unlock' })
      } else
        setError(e instanceof Error ? e.message : 'Payment could not be quoted.')
    } finally {
      submitting.current = false
    }
  }
  function unlocked(status: LockStatus) {
    lock.update(status)
    setStep(null)
    if (status.state === 'unlocked') void start(status)
  }
  async function send(ticket: Ticket) {
    setStep(null)
    if (submitting.current || blocked || !net) return
    submitting.current = true
    setSending(true)
    setError('')
    try {
      resource.update(await payOrder(id, net, ticket.ticket))
    } catch (e) {
      if (isLocked(e)) void lock.refresh()
      setError(e instanceof Error ? e.message : 'Payment could not be started.')
    } finally {
      await resource.refresh()
      await wallet.refresh()
      submitting.current = false
      setSending(false)
    }
  }
  const txids = [
    ...new Set([...order.txids, ...(order.payTxid ? [order.payTxid] : [])])
  ]
  return (
    <section
      className='ob-order-payment'
      aria-label={`Payment for ${order.seller}`}
    >
      <div className='ob-payment-progress' aria-live='polite'>
        <strong>{order.status}</strong>
        {monero ? (
          <span>
            Received {monero.receivedXmr} / {monero.amountXmr} XMR
          </span>
        ) : zcash ? (
          <span>
            Received {zcash.receivedZec} / {zcash.amountZec} ZEC
          </span>
        ) : (
          <span>
            Received {order.receivedSats.toLocaleString()} /{' '}
            {order.totalSats.toLocaleString()} sats
          </span>
        )}
        {kind === 'btc' && order.status !== ORDER_STATUS.declined && (
          <span>
            Still owed: {due.toLocaleString()} sats ({bitcoinAmount(due)} BTC)
          </span>
        )}
        <span>{view.confirmations}</span>
      </div>
      {order.status === ORDER_STATUS.declined && (
        <p role='alert' className='ob-form-error'>
          {('remote' in order && order.remote?.reason) || 'The seller declined this order.'} Nothing was paid.
        </p>
      )}
      {txids.map(txid => (
        <CopyPaymentValue
          key={txid}
          label={order.payTxid === txid ? 'Sent transaction' : 'Transaction'}
          value={txid}
        />
      ))}
      {txids.length > 0 && <p className='ob-wallet-note'>{EXTERNAL_TX_NOTE}</p>}
      {(error || order.payError || resource.error) && (
        <p role='alert' className='ob-form-error'>
          {error || order.payError || resource.error}
        </p>
      )}
      {directWarning && (
        <p role='note' className='ob-form-error'>
          {directWarning}
        </p>
      )}
      {escrow ? (
        <>
          {order.escrow?.moderator ? (
            <p className='ob-page-intro'>
              Moderator: {order.escrow.moderator.name || 'Unnamed moderator'}{order.escrow.moderator.builtIn ? ' (built-in test moderator)' : ''}, key <code title={order.escrow.moderator.id}>{order.escrow.moderator.id.slice(0, 8)}…{order.escrow.moderator.id.slice(-6)}</code>. A dispute on this order goes to this moderator only.
            </p>
          ) : null}
          {order.escrow?.moderatorFeeSats ? (
            <p className='ob-page-intro'>
              Moderator fee {order.escrow.moderatorFeeSats.toLocaleString()} sats, paid from the escrow to the moderator key in the terms.
            </p>
          ) : null}
          <p className='ob-page-intro'>
            Escrow payment is not available here yet. You can track this order’s status here.
          </p>
        </>
      ) : kind === 'xmr' ? (
        <>
          {view.progress && <p role='status'>{view.progress}</p>}
          {monero && monero.awaiting && order.status === ORDER_STATUS.awaitingPayment && !order.payTxid ? (
            <div className='ob-payment-options'>
              <PaymentQr value={monero.uri} />
              <CopyPaymentValue label='Monero subaddress' value={monero.subaddress} />
              <CopyPaymentValue label='Amount in XMR' value={monero.remainingXmr} />
              <p className='ob-page-intro'>
                Send only {monero.network} Monero to this subaddress. The QR code requests the remaining amount.
              </p>
            </div>
          ) : monero ? null : (
            <p className='ob-page-intro'>This Monero payment could not be read.</p>
          )}
        </>
      ) : kind === 'zec' ? (
        <>
          {view.progress && <p role='status'>{view.progress}</p>}
          {zcash?.expired ? (
            <p className='ob-form-error' role='alert'>
              This Zcash quote has expired. Do not pay it. Place the order again for a new one.
              {zcash.receivedZec !== '0' ? ` ${zcash.receivedZec} ZEC arrived and is recorded on this order, but the order was not marked paid.` : ''}
            </p>
          ) : zcash && zcash.awaiting && order.status === ORDER_STATUS.awaitingPayment ? (
            <div className='ob-payment-options'>
              <PaymentQr value={zcash.uri} />
              <CopyPaymentValue label='Zcash address' value={zcash.address} />
              <CopyPaymentValue label='Amount in ZEC' value={zcash.remainingZec} />
              <p className='ob-page-intro'>
                Send only {zcash.network} Zcash to this shielded address, from any Zcash wallet. It has no transparent receiver, so the wallet sends shielded and the payment lands in Ironwood. The QR code requests the remaining amount.
                {zcash.expiresAt != null ? ` The quote expires ${new Date(zcash.expiresAt * 1000).toLocaleTimeString(undefined, { timeStyle: 'short' })}.` : ''}
              </p>
              {sellerZcashNote(order) && <p className='ob-page-intro'>{sellerZcashNote(order)}</p>}
            </div>
          ) : zcash ? (
            zcash.overpaidZec && <p className='ob-page-intro'>{zcash.overpaidZec} ZEC more than the price arrived.</p>
          ) : (
            <p className='ob-page-intro'>This Zcash payment could not be read.</p>
          )}
        </>
      ) : (
        <>
          {mismatch && (
            <p className='ob-form-error' role='alert'>
              Order, wallet, and node networks must match before paying.
            </p>
          )}
          {(network.error || wallet.error) && (
            <p className='ob-form-error' role='alert'>
              {network.error || wallet.error}
            </p>
          )}
          {view.progress && <p role='status'>{view.progress}</p>}
          {offerBitcoinPay && (
            <div className='ob-payment-options'>
              <div className='ob-pay-wallet'>
                <p className='ob-rail'><RailMark rail='bitcoin' /> Bitcoin on-chain</p>
                <button
                  className='ob-primary'
                  disabled={blocked}
                  onClick={() => void start()}
                >
                  Pay from your node wallet
                </button>
                {view.balance && (
                  <p className='ob-pay-note is-blocking'>
                    {view.balance.message}{' '}
                    <button className='ob-text-link' onClick={onWallet}>
                      {view.balance.kind === 'pending'
                        ? 'Open wallet'
                        : 'Fund your node wallet'}
                    </button>
                  </p>
                )}
                {wallet.data?.syncError && (
                  <p className='ob-pay-note is-blocking'>
                    Wallet sync failed.{' '}
                    <button className='ob-text-link' onClick={onWallet}>
                      Open wallet
                    </button>
                  </p>
                )}
                <p className='ob-pay-note'>
                  This pays only {order.seller}. Network fees are additional.
                </p>
              </div>
              <details className='ob-external-payment'>
                <summary>Pay from another wallet</summary>
                {order.payment?.address &&
                  !mismatch &&
                  orderNetwork &&
                  net &&
                  !network.error && (
                    <>
                      <PaymentQr
                        value={bitcoinUri(order.payment.address, due)}
                      />
                      <CopyPaymentValue
                        label='Payment address'
                        value={order.payment.address}
                      />
                      <CopyPaymentValue
                        label='Amount in BTC'
                        value={bitcoinAmount(due)}
                      />
                      <p className='ob-page-intro'>
                        Send only{' '}
                        {orderNetwork === 'testnet4'
                          ? 'testnet4 coins'
                          : 'mainnet bitcoin'}{' '}
                        to this address. The QR code requests the remaining
                        amount.
                      </p>
                    </>
                  )}
              </details>
            </div>
          )}
        </>
      )}
      {kind === 'btc' &&
        orderNetwork &&
        order.payTxid &&
        order.confirmations === 0 &&
        (order.status === ORDER_STATUS.awaitingPayment ||
          order.status === ORDER_STATUS.paymentSeen) && (
          <SpeedUpPayment
            orderId={id}
            network={orderNetwork}
            lock={lock.data ?? null}
            onLock={status => lock.update(status)}
            onDone={() => void resource.refresh()}
          />
        )}
      {step?.kind === 'unlock' && lock.data && (
        <UnlockDialog
          status={lock.data}
          onCancel={() => setStep(null)}
          onUnlocked={unlocked}
        />
      )}
      {step?.kind === 'confirm' && (
        <SpendConfirmDialog
          quote={step.quote}
          seller={order.seller}
          onCancel={() => setStep(null)}
          onTicket={ticket => void send(ticket)}
        />
      )}
    </section>
  )
}
