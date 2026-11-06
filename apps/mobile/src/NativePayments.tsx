import { useMemo, useRef, useState } from 'react'
import {
  Linking,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View
} from 'react-native'
import * as Clipboard from 'expo-clipboard'
import QRCode from 'qrcode'
import {
  fetchOrder,
  type BuyerOrder,
  type CurrencyCode,
  type RateTable,
  type SellerOrder
} from './catalog'
import {
  amountDue,
  bitcoinAmount,
  bitcoinUri,
  newWalletAddress,
  payOrder,
  txUrl
} from './nodeWallet'
import { paymentStatusView } from './paymentStatus'
import { useNodeResource } from './useNodeResource'
import {
  isLocked,
  issueTicket,
  passphraseError,
  quotePayment,
  unlockWallet,
  type Quote
} from './walletLock'
import { usePairedNode } from './marketplace/NodePayments'

function Action({
  label,
  onPress,
  disabled = false
}: {
  label: string
  onPress: () => void
  disabled?: boolean
}) {
  return (
    <Pressable
      accessibilityRole='button'
      accessibilityLabel={label}
      disabled={disabled}
      onPress={onPress}
      style={[s.button, disabled && { opacity: 0.45 }]}
    >
      <Text style={s.buttonText}>{label}</Text>
    </Pressable>
  )
}
function Copy({ label, value }: { label: string; value: string }) {
  const [message, setMessage] = useState('')
  return (
    <View style={s.copy}>
      <Text style={s.label}>{label}</Text>
      <Text selectable style={s.code}>
        {value}
      </Text>
      <Action
        label={message || `Copy ${label.toLowerCase()}`}
        onPress={() => {
          void Clipboard.setStringAsync(value).then(
            () => setMessage('Copied'),
            () => setMessage('Copy failed; select the text above')
          )
        }}
      />
    </View>
  )
}
function Qr({ value }: { value: string }) {
  const matrix = useMemo(() => {
    try {
      return QRCode.create(value, { errorCorrectionLevel: 'M' }).modules
    } catch {
      return null
    }
  }, [value])
  if (!matrix)
    return <Text style={s.body}>QR unavailable. Copy the address instead.</Text>
  const unit = 224 / (matrix.size + 8),
    runs = []
  for (let row = 0; row < matrix.size; row++)
    for (let col = 0; col < matrix.size; col++) {
      if (!matrix.data[row * matrix.size + col]) continue
      const start = col
      while (col + 1 < matrix.size && matrix.data[row * matrix.size + col + 1])
        col++
      runs.push(
        <View
          key={`${row}-${start}`}
          style={{
            position: 'absolute',
            left: (start + 4) * unit,
            top: (row + 4) * unit,
            width: (col - start + 1) * unit + 0.1,
            height: unit + 0.1,
            backgroundColor: '#071824'
          }}
        />
      )
    }
  return (
    <View
      accessibilityLabel='Bitcoin payment QR code'
      accessibilityRole='image'
      style={{
        width: 224,
        height: 224,
        backgroundColor: 'white',
        marginVertical: 16
      }}
    >
      {runs}
    </View>
  )
}
function Amount({
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
    <View>
      <Text style={s.balance}>{sats.toLocaleString()} sats</Text>
      <Text style={s.body}>
        {bitcoinAmount(sats)} BTC
        {rate
          ? ` ≈ ${new Intl.NumberFormat('en-US', { style: 'currency', currency }).format((sats / 1e8) * rate)}`
          : ' · fiat estimate unavailable'}
      </Text>
    </View>
  )
}
export function NativeNetworkBadge() {
  const { network } = usePairedNode()
  return (
    <Text
      accessibilityLiveRegion='polite'
      style={[s.badge, network.data?.network === 'testnet4' && s.testnet]}
    >
      {network.error
        ? 'Network unavailable · payments disabled'
        : !network.data
          ? 'Checking bitcoin network…'
          : network.data.network === 'testnet4'
            ? 'Testnet4 · test coins, no value'
            : 'Mainnet'}
    </Text>
  )
}
export function NativeWallet({
  rates,
  currency
}: {
  rates: RateTable
  currency: CurrencyCode
}) {
  const { wallet, network, lock } = usePairedNode()
  const [error, setError] = useState(''),
    [busy, setBusy] = useState(false),
    [fresh, setFresh] = useState<{ network: string; address: string } | null>(
      null
    )
  async function next() {
    if (busy) return
    setBusy(true)
    setError('')
    try {
      setFresh(await newWalletAddress())
      await wallet.refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Address unavailable')
    } finally {
      setBusy(false)
    }
  }
  const current = wallet.data,
    mismatch =
      current && network.data && current.network !== network.data.network
  const address =
    fresh?.network === current?.network
      ? fresh?.address
      : current?.receiveAddress
  return (
    <ScrollView contentContainerStyle={s.page}>
      <Text style={s.heading}>Node wallet</Text>
      <Action label='Refresh wallet' onPress={() => void wallet.refresh()} />
      {lock.data?.state === 'setup' && (
        <Text style={s.error}>
          Set a wallet passphrase on the desktop or web wallet page. Until then
          this node will not spend.
        </Text>
      )}
      {error || wallet.error || network.error ? (
        <Text accessibilityRole='alert' style={s.error}>
          {error || wallet.error || network.error}
        </Text>
      ) : null}
      {current ? (
        <>
          <Text style={s.section}>Spendable</Text>
          <Amount
            sats={current.balance.spendableSats}
            rates={rates}
            currency={currency}
          />
          <Text style={s.section}>Pending</Text>
          <Amount
            sats={current.balance.pendingSats}
            rates={rates}
            currency={currency}
          />
          <Text style={s.section}>Receive bitcoin</Text>
          <Text style={s.body}>
            {current.network === 'testnet4'
              ? 'Fund with testnet4 coins from a faucet or test wallet.'
              : 'Fund with mainnet bitcoin.'}
          </Text>
          {mismatch ? (
            <Text style={s.error}>Wallet and node networks do not match.</Text>
          ) : address ? (
            <>
              <Qr value={bitcoinUri(address)} />
              <Copy label='Receive address' value={address} />
              <Action
                label={busy ? 'Creating address…' : 'New address'}
                disabled={busy || !!wallet.error || !!network.error}
                onPress={() => void next()}
              />
            </>
          ) : null}
          <Text style={s.body}>
            Last sync:{' '}
            {current.lastSync
              ? new Date(current.lastSync * 1000).toLocaleString()
              : 'Not synced yet'}
          </Text>
          {current.syncError && (
            <Text style={s.error}>Wallet sync: {current.syncError}</Text>
          )}
          {network.data?.publicChainServer && (
            <Text style={s.body}>
              The public chain server can see wallet addresses queried by this
              node.
            </Text>
          )}
        </>
      ) : (
        <Text style={s.body}>Loading node wallet…</Text>
      )}
    </ScrollView>
  )
}
export function NativeOrderPayment({
  initial
}: {
  initial: BuyerOrder | SellerOrder
}) {
  const id = 'id' in initial ? initial.id : initial.orderId
  const { network, wallet, lock: walletLock, onWallet } = usePairedNode()
  const resource = useNodeResource(
    () => fetchOrder(id),
    12000,
    true,
    o => o.status === 'Paid'
  )
  const order = resource.data ?? initial,
    net = network.data?.network,
    orderNet = order.network ?? order.payment?.network
  const due = amountDue(order.totalSats, order.receivedSats),
    escrow =
      order.settlement === 'escrow' ||
      !!order.escrow ||
      order.status === 'Awaiting vendor terms'
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(''),
    [step, setStep] = useState<'unlock' | Quote | null>(null),
    [passphrase, setPassphrase] = useState('')
  const lock = useRef(false)
  const view = paymentStatusView({
    network: orderNet ?? net ?? 'testnet4',
    status: order.status,
    confirmations: order.confirmations,
    requiredConfirmations: network.data?.confirmations,
    dueSats: due,
    sending: order.paying || busy,
    payTxid: order.payTxid,
    wallet: wallet.data?.balance
  })
  const insufficient = !!view.balance
  const mismatch = !!net && (orderNet !== net || wallet.data?.network !== net)
  const disabled =
    busy ||
    resource.loading ||
    !!resource.error ||
    !!network.error ||
    !!wallet.error ||
    !net ||
    !orderNet ||
    !wallet.data ||
    wallet.loading ||
    !!wallet.data.syncError ||
    mismatch ||
    insufficient ||
    order.paying ||
    !!order.payTxid ||
    due === 0 ||
    order.status !== 'Awaiting payment' ||
    escrow
  const current = useRef({ disabled, net })
  current.current = { disabled, net }
  async function run(task: () => Promise<void>, fallback: string) {
    if (lock.current) return
    lock.current = true
    setBusy(true)
    setError('')
    try {
      await task()
    } catch (e) {
      if (isLocked(e)) {
        void walletLock.refresh()
        setStep('unlock')
      }
      setError(passphraseError(e, fallback))
    } finally {
      lock.current = false
      setBusy(false)
    }
  }
  // Unlock if needed, then show the node's exact terms and ask again.
  function start() {
    if (current.current.disabled || !current.current.net) return
    if (walletLock.data?.state !== 'unlocked') {
      setStep('unlock')
      return
    }
    void run(async () => setStep(await quotePayment(id)), 'Payment could not be quoted')
  }
  function unlock() {
    void run(async () => {
      walletLock.update(await unlockWallet(passphrase))
      setPassphrase('')
      setStep(await quotePayment(id))
    }, 'The wallet could not be unlocked')
  }
  function confirm(quote: Quote) {
    const active = current.current.net
    if (!active) return
    void run(async () => {
      const ticket = await issueTicket(quote.quote, passphrase)
      setPassphrase('')
      setStep(null)
      try {
        resource.update(await payOrder(id, active, ticket.ticket))
      } finally {
        await resource.refresh()
        await wallet.refresh()
      }
    }, 'Payment failed')
  }
  const txids = [
    ...new Set([...order.txids, ...(order.payTxid ? [order.payTxid] : [])])
  ]
  return (
    <View style={s.payment}>
      <Text style={s.section}>{order.status}</Text>
      <Text style={s.body}>
        Received {order.receivedSats.toLocaleString()} /{' '}
        {order.totalSats.toLocaleString()} sats
      </Text>
      <Text style={s.body}>{view.confirmations}</Text>
      {txids.map(txid => (
        <Pressable
          key={txid}
          accessibilityRole='link'
          onPress={() => {
            if (orderNet) void Linking.openURL(txUrl(orderNet, txid))
          }}
        >
          <Text style={s.link}>Transaction: {txid}</Text>
        </Pressable>
      ))}
      {error ||
      order.payError ||
      resource.error ||
      wallet.error ||
      network.error ? (
        <Text style={s.error}>
          {error ||
            order.payError ||
            resource.error ||
            wallet.error ||
            network.error}
        </Text>
      ) : null}
      {escrow ? (
        <Text style={s.body}>Escrow status is read-only in this client.</Text>
      ) : (
        <>
          <Text style={s.body}>
            Still owed: {due.toLocaleString()} sats ({bitcoinAmount(due)} BTC)
          </Text>
          {mismatch && (
            <Text style={s.error}>
              Order, wallet, and node networks must match.
            </Text>
          )}
          {view.progress && <Text style={s.body}>{view.progress}</Text>}
          {due > 0 && !order.payTxid && order.status === 'Awaiting payment' && (
            <>
              {step === 'unlock' ? (
                <View style={s.copy}>
                  <Text style={s.section}>Unlock wallet</Text>
                  {walletLock.data?.state === 'setup' ? (
                    <Text style={s.body}>
                      Set a wallet passphrase on the desktop or web wallet page
                      first.
                    </Text>
                  ) : (
                    <>
                      <TextInput
                        accessibilityLabel='Wallet passphrase'
                        secureTextEntry
                        autoFocus
                        value={passphrase}
                        onChangeText={setPassphrase}
                        style={s.input}
                      />
                      <Action
                        label={busy ? 'Unlocking…' : 'Unlock wallet'}
                        disabled={busy || !passphrase}
                        onPress={unlock}
                      />
                    </>
                  )}
                  <Action label='Cancel' onPress={() => setStep(null)} />
                </View>
              ) : step ? (
                <View style={s.copy}>
                  <Text style={s.section}>
                    {step.network === 'mainnet'
                      ? 'Send real bitcoin?'
                      : 'Confirm payment'}
                  </Text>
                  <Text style={s.body}>
                    Pay {step.amountSats.toLocaleString()} sats (
                    {bitcoinAmount(step.amountSats)} BTC) to {order.seller}.
                    Network fee at most {step.feeCapSats.toLocaleString()} sats.
                  </Text>
                  <Text selectable style={s.code}>
                    {step.address}
                  </Text>
                  <Text style={s.body}>
                    Your node will refuse to send anything else. This expires in{' '}
                    {step.expiresInSecs} seconds.
                  </Text>
                  <TextInput
                    accessibilityLabel='Wallet passphrase'
                    secureTextEntry
                    autoFocus
                    value={passphrase}
                    onChangeText={setPassphrase}
                    style={s.input}
                  />
                  <Action
                    label={busy ? 'Confirming…' : 'Confirm payment'}
                    disabled={busy || !passphrase}
                    onPress={() => confirm(step)}
                  />
                  <Action label='Cancel' onPress={() => setStep(null)} />
                </View>
              ) : (
                <Action
                  label='Pay from your node wallet'
                  disabled={disabled}
                  onPress={start}
                />
              )}
              {view.balance && (
                <Text style={s.body}>{view.balance.message}</Text>
              )}
              <Action label='Open node wallet' onPress={onWallet} />
              {order.payment?.address && !mismatch && (
                <>
                  <Text style={s.section}>Pay from another wallet</Text>
                  <Qr value={bitcoinUri(order.payment.address, due)} />
                  <Copy label='Payment address' value={order.payment.address} />
                  <Copy label='Amount in BTC' value={bitcoinAmount(due)} />
                </>
              )}
            </>
          )}
        </>
      )}
    </View>
  )
}
const s = StyleSheet.create({
  page: { padding: 24, gap: 12 },
  heading: { fontSize: 30, fontWeight: '500', color: '#173b58' },
  section: {
    fontSize: 19,
    fontWeight: '500',
    color: '#173b58',
    marginTop: 20,
    marginBottom: 8
  },
  body: { fontSize: 15, lineHeight: 23, color: '#536779', marginVertical: 6 },
  balance: { fontSize: 27, color: '#173b58', marginTop: 8 },
  button: {
    backgroundColor: '#1062a9',
    minHeight: 44,
    justifyContent: 'center',
    alignSelf: 'flex-start',
    paddingHorizontal: 17,
    paddingVertical: 11,
    borderRadius: 5,
    marginVertical: 8
  },
  buttonText: { color: 'white', fontSize: 14 },
  badge: {
    fontSize: 13,
    backgroundColor: '#edf3f9',
    color: '#536779',
    padding: 10
  },
  testnet: { color: '#714b0a', backgroundColor: '#fff0c9' },
  error: { color: '#9f2f2f', fontSize: 15, lineHeight: 23, marginVertical: 12 },
  copy: { gap: 8, marginVertical: 12 },
  input: {
    borderWidth: 1,
    borderColor: '#dce4ed',
    borderRadius: 5,
    minHeight: 44,
    paddingHorizontal: 12,
    color: '#173b58'
  },
  label: { fontSize: 14, fontWeight: '500', color: '#173b58' },
  code: { fontSize: 13, lineHeight: 21, color: '#536779' },
  payment: {
    marginTop: 16,
    borderTopWidth: 1,
    borderTopColor: '#dce4ed',
    paddingTop: 10
  },
  link: { color: '#1062a9', fontSize: 13, lineHeight: 21, marginTop: 12 }
})
