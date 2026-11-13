import { useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import type { CurrencyCode, RateTable } from '../catalog'
import { fetchLnStatus, lnNetworkLabel, type LnStatus } from '../lightning'
import { fetchXmrRate, fetchXmrWallet, formatXmr, newXmrSubaddress, type XmrSubaddress, type XmrBackend, type XmrWallet } from '../xmr'
import type { ZecBackend } from '../zec'
import { useNodeResource } from '../useNodeResource'
import { CopyAddress, CopyButton } from './CopyAddress'
import { InfoTip } from './InfoTip'
import { CopyPaymentValue, PaymentQr, usePairedNode, WalletAmount, WalletPanel } from './NodePayments'
import { PageHeading } from './PageHeading'
import { RailMark, type Rail } from './RailMark'
import { Dialog, WalletLockPanel } from './WalletLock'

type Props = {
  rates: RateTable
  currency: CurrencyCode
  onOrders: () => void
  onSales?: () => void
  onManage: (rail: 'lightning' | 'monero' | 'zcash') => void
}
type WalletChoice = { rail: Rail; name: string; network: string; balance: string }

/** One workspace per enabled wallet. Balances on different networks are never added together. */
export function WalletPage({ rates, currency, onOrders, onSales, onManage }: Props) {
  const { wallet, network, lock } = usePairedNode()
  const lightning = useNodeResource(fetchLnStatus, 20000)
  const moneroEnabled = !!network.data?.xmrBackend && network.data.xmrBackend.backend !== 'none'
  const monero = useNodeResource(fetchXmrWallet, 20000, moneroEnabled)
  const moneroRate = useNodeResource(fetchXmrRate, 60000, moneroEnabled && monero.data?.network === 'mainnet')
  const [selected, setSelected] = useState<Rail>('bitcoin')
  const [settings, setSettings] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const tabs = useRef<HTMLDivElement>(null)
  const choices: WalletChoice[] = [{ rail: 'bitcoin', name: 'Bitcoin', network: wallet.data?.network === 'testnet4' ? 'Testnet4' : wallet.data?.network === 'mainnet' ? 'Mainnet' : 'On chain', balance: wallet.data ? `${wallet.data.balance.spendableSats.toLocaleString('en-US')} sats` : 'Balance unavailable' }]
  if (lightning.data?.enabled) choices.push({ rail: 'lightning', name: 'Lightning', network: lightning.data.network === 'signet' ? 'Mutinynet signet' : lightning.data.network === 'bitcoin' ? 'Mainnet' : lightning.data.network ?? 'Starting', balance: lightning.data.balances ? `${lightning.data.balances.lightningSats.toLocaleString('en-US')} sats` : 'Balance unavailable' })
  if (moneroEnabled) choices.push({ rail: 'monero', name: 'Monero', network: network.data!.xmrBackend!.network === 'stagenet' ? 'Stagenet' : 'Mainnet', balance: monero.data?.balance ? `${formatXmr(monero.data.balance.unlocked)} XMR` : monero.loading ? 'Loading balance…' : 'Balance unavailable' })
  if (network.data?.zecBackend?.enabled) choices.push({ rail: 'zcash', name: 'Zcash', network: network.data.zecBackend.network === 'testnet' ? 'Testnet' : 'Mainnet', balance: 'Shielded receiving' })
  const active = choices.some(choice => choice.rail === selected) ? selected : 'bitcoin'
  function keyDown(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const next = event.key === 'ArrowRight' ? (index + 1) % choices.length : event.key === 'ArrowLeft' ? (index + choices.length - 1) % choices.length : event.key === 'Home' ? 0 : event.key === 'End' ? choices.length - 1 : null
    if (next == null) return
    event.preventDefault()
    setSelected(choices[next].rail)
    tabs.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus()
  }
  async function refresh() {
    if (refreshing) return
    setRefreshing(true)
    try { await Promise.all([wallet.refresh(), network.refresh(), lightning.refresh(), monero.refresh(), moneroRate.refresh()]) }
    finally { setRefreshing(false) }
  }
  return <section className='ob-wallet-page ob-wallet-overview ob-wallet-hub'>
    <PageHeading title='Your wallet' description='Balances and payments across your enabled wallets.' action={<div className='ob-wallet-heading-actions'>
      {choices.length > 1 && <button type='button' className='ob-wallet-refresh' aria-label='Refresh wallets' title='Refresh wallets' disabled={refreshing} onClick={() => void refresh()}><svg width='18' height='18' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.6' aria-hidden='true'><path d='M20 7v5h-5M4 17v-5h5'/><path d='M6.1 7a7 7 0 0 1 11.7-1L20 9M4 15l2.2 3a7 7 0 0 0 11.7-1'/></svg><span className='ob-sr-only' role='status'>{refreshing ? 'Refreshing wallets…' : ''}</span></button>}
      <button type='button' className='ob-secondary' aria-label='Wallet settings' onClick={() => setSettings(true)}>Settings</button>
    </div>}/>
    {choices.length > 1 && <div className='ob-wallet-selector' role='tablist' aria-label='Wallets' ref={tabs}>
      {choices.map((choice, index) => <button type='button' role='tab' id={`ob-wallet-tab-${choice.rail}`} aria-controls={`ob-wallet-panel-${choice.rail}`} aria-selected={active === choice.rail} tabIndex={active === choice.rail ? 0 : -1} key={choice.rail} onClick={() => setSelected(choice.rail)} onKeyDown={event => keyDown(event, index)}>
        <span className='ob-wallet-choice-name'><RailMark rail={choice.rail}/>{choice.name}<span className='ob-wallet-choice-check' aria-hidden='true'>✓</span></span>
        <strong title={choice.balance}>{choice.balance}</strong><span className='ob-wallet-choice-network'>{choice.network}</span>
      </button>)}
    </div>}
    {lightning.error && !lightning.data && <p className='ob-wallet-discovery-error' role='status'>Lightning status is unavailable. Refresh wallets to try again.</p>}
    <div id='ob-wallet-panel-bitcoin' className='ob-wallet-tabpanel' role={choices.length > 1 ? 'tabpanel' : undefined} aria-labelledby={choices.length > 1 ? 'ob-wallet-tab-bitcoin' : undefined} hidden={active !== 'bitcoin'}><WalletPanel rates={rates} currency={currency} onOrders={onOrders} embedded/></div>
    {lightning.data?.enabled && <div id='ob-wallet-panel-lightning' className='ob-wallet-tabpanel' role='tabpanel' aria-labelledby='ob-wallet-tab-lightning' hidden={active !== 'lightning'}>
      <LightningPanel ln={lightning.data} error={lightning.error} rates={rates} currency={currency} onManage={() => onManage('lightning')} onOrders={onOrders}/>
    </div>}
    {moneroEnabled && <div id='ob-wallet-panel-monero' className='ob-wallet-tabpanel' role='tabpanel' aria-labelledby='ob-wallet-tab-monero' hidden={active !== 'monero'}>
      <MoneroPanel resource={monero} backend={network.data!.xmrBackend!} fiatRate={moneroRate.data?.currencies[currency]} currency={currency} onManage={() => onManage('monero')} onOrders={onOrders}/>
    </div>}
    {network.data?.zecBackend?.enabled && <div id='ob-wallet-panel-zcash' className='ob-wallet-tabpanel' role='tabpanel' aria-labelledby='ob-wallet-tab-zcash' hidden={active !== 'zcash'}>
      <ZcashPanel backend={network.data.zecBackend} onManage={() => onManage('zcash')} onSales={onSales}/>
    </div>}
    {settings && <Dialog label='Wallet settings' onCancel={() => setSettings(false)}><h2>Wallet settings</h2><WalletLockPanel lock={lock}/><div className='ob-form-actions'><button type='button' className='ob-secondary' onClick={() => setSettings(false)}>Done</button></div></Dialog>}
  </section>
}

function WalletHeader({ rail, title, network, status, attention, action }: { rail: Rail; title: string; network: string; status: string; attention?: boolean; action?: ReactNode }) {
  return <header className='ob-wallet-detail-heading'><div><h2><RailMark rail={rail}/>{title}</h2><p>{network}</p></div><div className='ob-wallet-detail-actions'><span className={`ob-wallet-state ${attention ? 'needs-attention' : ''}`}><span className='ob-status-dot' aria-hidden='true'/>{status}</span>{action}</div></header>
}
function Metric({ title, children, note, help }: { title: string; children: ReactNode; note?: ReactNode; help?: ReactNode }) {
  return <article className='ob-wallet-metric'><h3>{title}{help && <InfoTip label={`About ${title.toLowerCase()}`}>{help}</InfoTip>}</h3>{children}{note && <p>{note}</p>}</article>
}

function LightningPanel({ ln, error, rates, currency, onManage, onOrders }: { ln: LnStatus; error: string; rates: RateTable; currency: CurrencyCode; onManage: () => void; onOrders: () => void }) {
  const usable = ln.channels.filter(channel => channel.usable).length
  return <div className='ob-wallet-detail'>
    <section className='ob-wallet-asset-summary' aria-label='Lightning balances'>
      <WalletHeader rail='lightning' title='Lightning' network={ln.network ? lnNetworkLabel(ln.network) : 'Network unavailable'} status={error ? 'Last known balance' : ln.error ? 'Needs attention' : ln.running ? 'Running' : ln.keys === 'unlock-to-create' ? 'Setup required' : 'Starting'} attention={!!error || !!ln.error || !ln.running}/>
      <div className='ob-wallet-metrics'>
        <Metric title='Available to spend' note='For purchases over Lightning.'>{ln.balances ? <WalletAmount sats={ln.balances.lightningSats} rates={rates} currency={currency} showFiat={ln.network === 'bitcoin'}/> : <p className='ob-wallet-unavailable'>Balance unavailable</p>}</Metric>
        <Metric title='Receiving capacity' note='Room for incoming Lightning payments.' help='Receiving capacity is channel liquidity, not a wallet balance. Incoming payments use this capacity; opening or rebalancing channels can change it.'>{ln.balances ? <WalletAmount sats={ln.balances.inboundSats} rates={rates} currency={currency} showFiat={ln.network === 'bitcoin'}/> : <p className='ob-wallet-unavailable'>Capacity unavailable</p>}</Metric>
      </div>
      <div className='ob-wallet-asset-footer'><button type='button' className='ob-primary' onClick={onOrders}>Your orders</button><button type='button' className='ob-secondary' onClick={onManage}>Manage channels</button><span>Invoices are created for each order.</span></div>
      {(error || ln.error) && <p className='ob-wallet-sync-error' role='alert'>{error || ln.error}{ln.balances ? ' Showing the last available balances.' : ''}</p>}
    </section>
    <div className='ob-wallet-support-row'><div><span>Channel funds on chain <InfoTip label='About channel funds'>Bitcoin held by the Lightning node for channel funding or returned from closed channels. These funds are separate from your spendable Lightning balance and your Bitcoin wallet.</InfoTip></span><strong>{ln.balances ? `${ln.balances.onchainSats.toLocaleString('en-US')} sats` : 'Unavailable'}</strong></div><div><span>Channels</span><strong>{usable} usable <small>of {ln.channels.length}</small></strong></div></div>
    <section className='ob-wallet-channel-list' aria-labelledby='ob-wallet-channels'><header><h2 id='ob-wallet-channels'>Channels</h2><span>{ln.lsp?.connected ? 'LSP connected' : ln.lsp ? 'LSP disconnected' : 'Direct channels'}</span></header>
      {ln.channels.length ? <ul>{ln.channels.map(channel => <li key={channel.id}><div className='ob-wallet-channel-peer'><span>Peer</span><code title={channel.peer}>{channel.peer.slice(0, 10)}…{channel.peer.slice(-6)}</code><CopyButton value={channel.peer} label='Copy peer ID'/></div><dl><div><dt>Can spend</dt><dd>{channel.outboundSats.toLocaleString('en-US')} sats</dd></div><div><dt>Can receive</dt><dd>{channel.inboundSats.toLocaleString('en-US')} sats</dd></div></dl><span className={`ob-wallet-state ${channel.usable ? '' : 'needs-attention'}`}>{channel.usable ? 'Usable' : channel.ready ? 'Not usable' : 'Opening'}</span></li>)}</ul> : <p className='ob-wallet-empty'>{ln.keys === 'unlock-to-create' ? 'Unlock your wallet in Settings to finish Lightning setup.' : 'No channels yet. Manage channels to connect this wallet.'}</p>}
    </section>
    <details className='ob-wallet-connection'><summary>Connection and privacy<span>Lightning details</span></summary><div className='ob-wallet-detail-disclosure'>{ln.nodeId && <CopyAddress value={ln.nodeId} label='Copy Lightning node ID'/>}{ln.reveals.map(line => <p key={line}>{line}</p>)}</div></details>
  </div>
}

function MoneroPanel({ resource, backend, fiatRate, currency, onManage, onOrders }: { resource: ReturnType<typeof useNodeResource<XmrWallet>>; backend: XmrBackend; fiatRate?: number; currency: CurrencyCode; onManage: () => void; onOrders: () => void }) {
  const current = resource.data
  const [fresh, setFresh] = useState<XmrSubaddress | null>(null)
  const [receive, setReceive] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const mismatch = !!current && current.network !== backend.network
  const address = fresh?.network === current?.network ? fresh?.address : current?.primaryAddress
  const stale = !!resource.error || !!current?.syncError || !!backend.error
  const receiveBlocked = !address || stale || mismatch || !current?.enabled || current.keys !== 'ready'
  async function newAddress() {
    if (busy || mismatch || !current?.enabled || current.keys !== 'ready') return
    setBusy(true); setError('')
    try {
      const next = await newXmrSubaddress()
      if (next.network !== current.network) throw new Error('The node returned an address for a different Monero network.')
      setFresh(next)
    }
    catch (e) { setError(e instanceof Error ? e.message : 'A receive address could not be created.') }
    finally { setBusy(false) }
  }
  function amount(value: bigint) {
    return <div className='ob-wallet-amount'><strong>{formatXmr(value)} <small>XMR</small></strong><span>{current?.network === 'stagenet' ? 'Test coins · no monetary value' : fiatRate ? `≈ ${new Intl.NumberFormat('en-US', { style: 'currency', currency }).format(Number(formatXmr(value)) * fiatRate)}` : 'Fiat estimate unavailable'}</span></div>
  }
  return <div className='ob-wallet-detail'>
    <section className='ob-wallet-asset-summary' aria-label='Monero balances'>
      <WalletHeader rail='monero' title='Monero' network={backend.network === 'stagenet' ? 'Stagenet · test coins, no value' : 'Mainnet'} status={mismatch ? 'Network mismatch' : stale ? 'Needs attention' : current?.keys === 'unlock-to-create' ? 'Setup required' : !current?.lastSync ? 'Syncing' : current.tip != null && current.scannedHeight != null && current.scannedHeight < current.tip ? 'Syncing' : 'Synced'} attention={mismatch || stale || !current?.lastSync}/>
      <div className='ob-wallet-metrics'><Metric title={stale ? 'Last known spendable balance' : 'Available to spend'}>{current?.balance ? amount(current.balance.unlocked) : <p className='ob-wallet-unavailable'>{resource.loading ? 'Loading balance…' : 'Balance unavailable'}</p>}</Metric><Metric title='Pending confirmation' note='Available after the Monero unlock period.'>{current?.balance ? amount(current.balance.pending) : <p className='ob-wallet-unavailable'>Balance unavailable</p>}</Metric></div>
      <div className='ob-wallet-asset-footer'><button type='button' className='ob-primary' disabled={receiveBlocked} onClick={() => setReceive(true)}>Receive Monero</button><button type='button' className='ob-secondary' onClick={onOrders}>Your orders</button><span>{current?.lastSync ? `Last synced ${new Date(current.lastSync * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}` : 'Not synced yet'}</span></div>
      {(resource.error || current?.syncError || backend.error || mismatch) && <p className='ob-wallet-sync-error' role='alert'>{mismatch ? 'Wallet and Monero networks do not match. Receiving is disabled.' : resource.error || current?.syncError || backend.error}{stale && current?.balance ? ' Showing the last available balance.' : ''}</p>}
    </section>
    <section className='ob-wallet-asset-receive'><header><div><h2>Receive Monero</h2><p>Use a fresh subaddress to keep deposits separate.</p></div><button type='button' className='ob-text-link' onClick={onManage}>Monero settings</button></header>
      {address && !mismatch ? <><CopyPaymentValue label='Receive address' value={address}/><div className='ob-wallet-receive-actions'><button type='button' className='ob-secondary' disabled={receiveBlocked} onClick={() => setReceive(true)}>Show QR code</button><button type='button' className='ob-secondary' disabled={busy || receiveBlocked} onClick={() => void newAddress()}>{busy ? 'Creating address…' : 'New subaddress'}</button></div></> : <p className='ob-wallet-empty'>{current?.keys === 'unlock-to-create' ? 'Unlock your wallet in Settings to create the Monero keys and receive address.' : 'A receive address will appear when this wallet is ready.'}</p>}
      {error && <p className='ob-form-error' role='alert'>{error}</p>}
    </section>
    <details className='ob-wallet-connection'><summary>Connection and privacy<span>Monero details</span></summary><div className='ob-wallet-detail-disclosure'><p>{backend.reveals}</p><p>{current?.scannedHeight != null ? `Scanned block ${current.scannedHeight.toLocaleString()}${current.tip != null ? ` of ${current.tip.toLocaleString()}` : ''}.` : 'Waiting for the first sync.'}</p></div></details>
    {receive && address && !receiveBlocked && <Dialog label='Receive Monero' onCancel={() => setReceive(false)}><div className='ob-wallet-receive-dialog'><h2>Receive Monero</h2><p>Monero {current?.network} only</p><PaymentQr value={`monero:${address}`} label='Monero receive QR code' paper/><CopyAddress value={address} label='Copy Monero receive address'/></div><div className='ob-form-actions'><button type='button' className='ob-secondary' onClick={() => setReceive(false)}>Done</button></div></Dialog>}
  </div>
}

function ZcashPanel({ backend, onManage, onSales }: { backend: ZecBackend; onManage: () => void; onSales?: () => void }) {
  const behind = backend.tip != null && backend.scannedHeight != null ? Math.max(0, backend.tip - backend.scannedHeight) : null
  return <div className='ob-wallet-detail'>
    <section className='ob-wallet-asset-summary' aria-label='Zcash receiving'>
      <WalletHeader rail='zcash' title='Zcash' network={backend.network === 'testnet' ? 'Testnet · test coins, no value' : 'Mainnet'} status={backend.error ? 'Needs attention' : behind === 0 ? 'Synced' : 'Syncing'} attention={!!backend.error || behind !== 0}/>
      <div className='ob-wallet-receiving-state'><div><span className='ob-wallet-capability'>Shielded order payments</span><h3>Receive through your store</h3><p>Each order gets its own shielded address. Track received amounts and confirmations in Sales.</p><p className='ob-wallet-capability-note'>A wallet balance and general receive address aren’t available here yet. Pay for Zcash purchases from another wallet.</p></div><dl><div><dt>Scanned block</dt><dd>{backend.scannedHeight?.toLocaleString() ?? 'Not synced yet'}</dd></div><div><dt>Chain tip</dt><dd>{backend.tip?.toLocaleString() ?? 'Unavailable'}</dd></div></dl></div>
      <div className='ob-wallet-asset-footer'>{onSales && <button type='button' className='ob-primary' onClick={onSales}>View sales</button>}<button type='button' className='ob-secondary' onClick={onManage}>Zcash settings</button></div>
      {backend.error && <p className='ob-wallet-sync-error' role='alert'>{backend.error}</p>}
    </section>
    <details className='ob-wallet-connection'><summary>Connection and privacy<span>Zcash details</span></summary><div className='ob-wallet-detail-disclosure'><p>{backend.reveals}</p></div></details>
  </div>
}
