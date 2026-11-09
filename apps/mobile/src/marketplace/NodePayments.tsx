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
  type NetworkInfo
} from '../nodeWallet'
import { paymentStatusView } from '../paymentStatus'
import { useNodeResource } from '../useNodeResource'
import { isLocked, quotePayment, type LockStatus, type Quote, type Ticket } from '../walletLock'
import { RailMark } from './RailMark'
import { SpendConfirmDialog, UnlockDialog, WalletLockPanel, useLockState } from './WalletLock'
import { PageHeading } from './PageHeading'
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
  const state = network.error
    ? 'is-offline'
    : !network.data
      ? 'is-loading'
      : network.data.network === 'testnet4'
        ? 'is-testnet'
        : 'is-mainnet'
  return (
    <div className={`ob-network-badge ${state}`} role='status'>
      <span className='ob-status-dot' aria-hidden='true' />
      <span>
        {network.error
          ? 'Network unavailable · payments disabled'
          : !network.data
            ? 'Checking bitcoin network…'
            : network.data.network === 'testnet4'
              ? 'Testnet4 · test coins, no value'
              : 'Mainnet'}
      </span>
      {network.data && privacyStatus(network.data) ? (
        <span>{privacyStatus(network.data)}</span>
      ) : null}
    </div>
  )
}
function PrivacyNotice({ info }: { info: NetworkInfo }) {
  const status = privacyStatus(info)
  const backend = info.chainBackend
  if (!status && !info.privacyWarning && !backend?.reveals && !backend?.error)
    return null
  return (
    <p className='ob-wallet-note' role='status'>
      {status ? `${status}. ` : null}
      {info.privacyWarning}
      {backend?.reveals ? ` ${backend.reveals}` : null}
      {backend?.error ? ` ${backend.error}` : null}
      {backend?.fallbackActive
        ? ` Public fallback${backend.fallback ? `: ${backend.fallback}` : ''} is active.`
        : null}
      {info.proxy
        ? ` Configured proxy: ${info.proxy}. A configured proxy is not a verified connection.`
        : null}
    </p>
  )
}
export function WalletAmount({
  sats,
  rates,
  currency
}: {
  sats: number
  rates: RateTable
  currency: CurrencyCode
}) {
  const rate = rates[currency]
  return (
    <div className='ob-wallet-amount'>
      <strong>
        {sats.toLocaleString('en-US')} <small>sats</small>
      </strong>
      <span>
        {bitcoinAmount(sats)} BTC
        {rate
          ? ` ≈ ${new Intl.NumberFormat('en-US', { style: 'currency', currency }).format((sats / 1e8) * rate)}`
          : ' · fiat estimate unavailable'}
      </span>
    </div>
  )
}
export function CopyPaymentValue({
  label,
  value,
  children
}: {
  label: string
  value: string
  children?: ReactNode
}) {
  const [message, setMessage] = useState('')
  return (
    <div className='ob-copy-value'>
      <span>{label}</span>
      <code>{value}</code>
      <div className='ob-copy-actions'>
        <button
          className='ob-text-link'
          type='button'
          onClick={() => {
            void navigator.clipboard.writeText(value).then(
              () => setMessage('Copied'),
              () => setMessage('Copy the text above manually')
            )
          }}
        >
          {message || `Copy ${label.charAt(0).toLowerCase()}${label.slice(1)}`}
        </button>
        {children}
      </div>
    </div>
  )
}
export function PaymentQr({ value }: { value: string }) {
  const [src, setSrc] = useState('')
  const [error, setError] = useState(false)
  useEffect(() => {
    let active = true
    setSrc('')
    setError(false)
    void QRCode.toDataURL(value, {
      width: 224,
      margin: 4,
      errorCorrectionLevel: 'M',
      color: { dark: '#071824', light: '#ffffff' }
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
  }, [value])
  return error ? (
    <p role='alert'>QR code unavailable. Copy the address instead.</p>
  ) : src ? (
    <img
      className='ob-payment-qr'
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
export function WalletPanel({
  rates,
  currency
}: {
  rates: RateTable
  currency: CurrencyCode
}) {
  const { wallet, network, lock } = usePairedNode()
  const [busy, setBusy] = useState(false),
    [error, setError] = useState('')
  const [freshAddress, setFreshAddress] = useState<{
    network: string
    address: string
  } | null>(null)
  async function nextAddress() {
    if (busy) return
    setBusy(true)
    setError('')
    try {
      const next = await newWalletAddress()
      setFreshAddress(next)
      await wallet.refresh()
    } catch (e) {
      setError(
        e instanceof Error ? e.message : 'A new address could not be created.'
      )
    } finally {
      setBusy(false)
    }
  }
  const current = wallet.data
  const mismatch =
    current && network.data && current.network !== network.data.network
  const address =
    current && freshAddress?.network === current.network
      ? freshAddress.address
      : current?.receiveAddress
  return (
    <section className='ob-wallet-page'>
      <PageHeading title="Your wallet" description="Your node holds this bitcoin and pays each seller separately." action={
        <button className='ob-secondary' disabled={wallet.loading} onClick={() => void wallet.refresh()}>Refresh wallet</button>
      }/>
      <WalletLockPanel lock={lock} />
      {(error || wallet.error || network.error) && (
        <p className='ob-form-error' role='alert'>
          {error || wallet.error || network.error}
        </p>
      )}
      {mismatch && (
        <p className='ob-form-error' role='alert'>
          Wallet and node networks do not match. Payments are disabled.
        </p>
      )}
      {network.data && <PrivacyNotice info={network.data} />}
      {!current ? (
        <p className='ob-page-intro' role='status'>
          {wallet.loading
            ? 'Loading node wallet…'
            : 'Wallet unavailable. Try refreshing.'}
        </p>
      ) : (
        <>
          <div className='ob-wallet-balances'>
            <article>
              <h2>Spendable</h2>
              <WalletAmount
                sats={current.balance.spendableSats}
                rates={rates}
                currency={currency}
              />
            </article>
            <article className='is-pending'>
              <h2>Pending</h2>
              <WalletAmount
                sats={current.balance.pendingSats}
                rates={rates}
                currency={currency}
              />
            </article>
            <div
              className={`ob-wallet-sync ${current.syncError ? 'is-failed' : ''}`}
            >
              <p>
                <span
                  className={`ob-status-dot ${current.syncError ? 'is-offline' : current.lastSync ? '' : 'is-loading'}`}
                  aria-hidden='true'
                />
                {current.lastSync
                  ? `Last synced ${new Date(current.lastSync * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}`
                  : 'Not synced yet'}
              </p>
              {current.syncError && (
                <p role='alert'>Wallet sync failed: {current.syncError}</p>
              )}
            </div>
          </div>
          <section className='ob-wallet-fund'>
            <h2>Receive bitcoin</h2>
            <p>
              {current.network === 'testnet4'
                ? 'Send testnet4 coins from a faucet or another testnet4 wallet.'
                : 'Send mainnet bitcoin from another wallet.'}
            </p>
            {address && !mismatch && (
              <div className='ob-wallet-receive'>
                <PaymentQr value={bitcoinUri(address)} />
                <CopyPaymentValue label='Receive address' value={address}>
                  <button
                    className='ob-text-link'
                    disabled={busy || !!wallet.error || !!network.error}
                    onClick={() => void nextAddress()}
                  >
                    {busy ? 'Creating address…' : 'New address'}
                  </button>
                </CopyPaymentValue>
              </div>
            )}
          </section>
        </>
      )}
    </section>
  )
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
    o => o.status === 'Paid'
  )
  const order = resource.data ?? initial
  const due = amountDue(order.totalSats, order.receivedSats)
  const net = network.data?.network
  const orderNetwork = order.network ?? order.payment?.network
  const mismatch =
    !!net &&
    ((!!orderNetwork && orderNetwork !== net) ||
      (!!wallet.data && wallet.data.network !== net))
  const escrow =
    order.settlement === 'escrow' ||
    !!order.escrow ||
    order.status === 'Awaiting vendor terms'
  const view = paymentStatusView({
    network: orderNetwork ?? net ?? 'testnet4',
    status: order.status,
    confirmations: order.confirmations,
    requiredConfirmations: network.data?.confirmations,
    dueSats: due,
    sending: order.paying || sending,
    payTxid: order.payTxid,
    wallet: wallet.data?.balance
  })
  const lowBalance = !!view.balance
  const blocked =
    escrow ||
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
    order.paying ||
    !!order.payTxid ||
    due === 0 ||
    order.status !== 'Awaiting payment'
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
        <span>
          Received {order.receivedSats.toLocaleString()} /{' '}
          {order.totalSats.toLocaleString()} sats
        </span>
        {!escrow && order.status !== 'Declined by the seller' && (
          <span>
            Still owed: {due.toLocaleString()} sats ({bitcoinAmount(due)} BTC)
          </span>
        )}
        <span>{view.confirmations}</span>
      </div>
      {order.status === 'Declined by the seller' && (
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
      {escrow ? (
        <>
          {order.escrow?.moderatorFeeSats ? (
            <p className='ob-page-intro'>
              Moderator fee {order.escrow.moderatorFeeSats.toLocaleString()} sats, paid from the escrow to the moderator key in the terms.
            </p>
          ) : null}
          <p className='ob-page-intro'>
            Escrow payment is not available here yet. You can track this order’s status here.
          </p>
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
          {due > 0 && !order.payTxid && order.status === 'Awaiting payment' && (
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
      {!escrow &&
        orderNetwork &&
        order.payTxid &&
        order.confirmations === 0 &&
        (order.status === 'Awaiting payment' ||
          order.status === 'Payment seen') && (
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
