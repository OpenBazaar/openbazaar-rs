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
  txUrl
} from '../nodeWallet'
import { useNodeResource } from '../useNodeResource'

function usePaymentState(activeWallet: boolean, onWallet: () => void) {
  const network = useNodeResource(fetchNetwork, 30000)
  const wallet = useNodeResource(fetchWallet, 15000, activeWallet)
  return { network, wallet, onWallet }
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
  return (
    <div
      className={`ob-network-badge ${network.data?.network === 'testnet4' ? 'is-testnet' : ''}`}
      role='status'
    >
      {network.error
        ? 'Network unavailable · payments disabled'
        : !network.data
          ? 'Checking bitcoin network…'
          : network.data.network === 'testnet4'
            ? 'Testnet4 · test coins, no value'
            : 'Mainnet'}
    </div>
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
  value
}: {
  label: string
  value: string
}) {
  const [message, setMessage] = useState('')
  return (
    <div className='ob-copy-value'>
      <span>{label}</span>
      <code>{value}</code>
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
        {message || `Copy ${label.toLowerCase()}`}
      </button>
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
  const { wallet, network } = usePairedNode()
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
      <div className='ob-section-heading'>
        <h1>Node wallet</h1>
        <button className='ob-secondary' onClick={() => void wallet.refresh()}>
          Refresh wallet
        </button>
      </div>
      <p className='ob-page-intro'>
        Fund this wallet to pay individual seller orders in the app. Balances
        refresh every 15 seconds.
      </p>
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
      {!current ? (
        <p role='status'>
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
            <article>
              <h2>Pending</h2>
              <WalletAmount
                sats={current.balance.pendingSats}
                rates={rates}
                currency={currency}
              />
            </article>
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
                <div>
                  <CopyPaymentValue label='Receive address' value={address} />
                  <button
                    className='ob-secondary'
                    disabled={busy || !!wallet.error || !!network.error}
                    onClick={() => void nextAddress()}
                  >
                    {busy ? 'Creating address…' : 'New address'}
                  </button>
                </div>
              </div>
            )}
          </section>
          <p className='ob-page-intro'>
            Last sync:{' '}
            {current.lastSync
              ? new Date(current.lastSync * 1000).toLocaleString()
              : 'Not synced yet'}
          </p>
          {current.syncError && (
            <p className='ob-form-error' role='alert'>
              Wallet sync: {current.syncError}
            </p>
          )}
          {network.data?.publicChainServer && (
            <p className='ob-page-intro'>
              This node uses a public chain server, which can see the wallet
              addresses it queries.
            </p>
          )}
        </>
      )}
    </section>
  )
}
function MainnetConfirmation({
  amount,
  seller,
  address,
  onCancel,
  onConfirm
}: {
  amount: number
  seller: string
  address: string
  onCancel: () => void
  onConfirm: () => void
}) {
  const dialog = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    dialog.current?.showModal()
    return () => {
      dialog.current?.close()
      previous?.focus()
    }
  }, [])
  return (
    <dialog
      ref={dialog}
      className='ob-pay-confirm'
      aria-labelledby='ob-pay-confirm-title'
      onCancel={e => {
        e.preventDefault()
        onCancel()
      }}
    >
      <h2 id='ob-pay-confirm-title'>Send real bitcoin?</h2>
      <p>
        This is a mainnet payment of{' '}
        <strong>
          {amount.toLocaleString()} sats ({bitcoinAmount(amount)} BTC)
        </strong>{' '}
        to {seller}, plus the network fee chosen by your node. Bitcoin payments
        cannot be reversed.
      </p>
      <code>{address}</code>
      <div className='ob-form-actions'>
        <button className='ob-secondary' autoFocus onClick={onCancel}>
          Cancel
        </button>
        <button className='ob-primary' onClick={onConfirm}>
          Confirm mainnet payment
        </button>
      </div>
    </dialog>
  )
}
export function OrderPayment({
  id,
  initial
}: {
  id: string
  initial: BuyerOrder | SellerOrder
}) {
  const { network, wallet, onWallet } = usePairedNode()
  const [error, setError] = useState(''),
    [sending, setSending] = useState(false),
    [confirming, setConfirming] = useState(false)
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
  const lowBalance = !!wallet.data && wallet.data.balance.spendableSats < due
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
  async function send(confirmed = false) {
    if (submitting.current || blocked || !net) {
      setConfirming(false)
      return
    }
    if (net === 'mainnet' && !confirmed) {
      setConfirming(true)
      return
    }
    submitting.current = true
    setSending(true)
    setConfirming(false)
    setError('')
    try {
      resource.update(await payOrder(id, net, confirmed))
    } catch (e) {
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
        {!escrow && (
          <span>
            Still owed: {due.toLocaleString()} sats ({bitcoinAmount(due)} BTC)
          </span>
        )}
        <span>
          Confirmations: {order.confirmations} /{' '}
          {network.data?.confirmations ?? '—'}
        </span>
      </div>
      {txids.map(txid =>
        orderNetwork ? (
          <a
            className='ob-transaction-link'
            key={txid}
            href={txUrl(orderNetwork, txid)}
            target='_blank'
            rel='noreferrer noopener'
          >
            {order.payTxid === txid ? 'Sent transaction: ' : 'Transaction: '}
            {txid}
          </a>
        ) : (
          <code key={txid}>{txid}</code>
        )
      )}
      {(error || order.payError || resource.error) && (
        <p role='alert' className='ob-form-error'>
          {error || order.payError || resource.error}
        </p>
      )}
      {escrow ? (
        <p className='ob-page-intro'>
          Escrow payment is not available in this client. This order’s status is
          read-only.
        </p>
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
          {order.paying || sending ? (
            <p role='status'>Sending payment from your node wallet…</p>
          ) : order.payTxid ? (
            <p role='status'>
              Payment sent. Waiting for the node to observe and confirm it.
            </p>
          ) : null}
          {due > 0 && !order.payTxid && order.status === 'Awaiting payment' && (
            <div className='ob-payment-options'>
              <div>
                <h3>Pay from your node wallet</h3>
                <button
                  className='ob-primary'
                  disabled={blocked}
                  onClick={() => void send()}
                >
                  Pay from your node wallet
                </button>
                {lowBalance && (
                  <p className='ob-page-intro'>
                    Insufficient spendable balance.{' '}
                    <button className='ob-text-link' onClick={onWallet}>
                      Fund your node wallet
                    </button>
                  </p>
                )}
                {wallet.data?.syncError && (
                  <p className='ob-page-intro'>
                    Wallet sync failed.{' '}
                    <button className='ob-text-link' onClick={onWallet}>
                      Open wallet
                    </button>
                  </p>
                )}
                <p className='ob-page-intro'>
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
      {confirming && net === 'mainnet' && (
        <MainnetConfirmation
          amount={due}
          seller={order.seller}
          address={order.payment?.address ?? ''}
          onCancel={() => setConfirming(false)}
          onConfirm={() => void send(true)}
        />
      )}
    </section>
  )
}
