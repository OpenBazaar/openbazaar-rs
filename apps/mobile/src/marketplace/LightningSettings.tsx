import { useState, type FormEvent } from 'react'
import {
  closeLnChannel,
  fetchLnStatus,
  fetchLnStores,
  lnNetworkLabel,
  openLnChannel,
  setStoreAcceptsLightning,
  type LnChannel,
  type LnStatus,
  type LnStores
} from '../lightning'
import { useNodeResource } from '../useNodeResource'
import { RailMark } from './RailMark'
import { Dialog, FormError, Passphrase, useSubmit } from './WalletLock'
import { CopyAddress } from './CopyAddress'

/** Seller settings: which stores take Lightning, the LSP, and channels. */
export function LightningSettings() {
  const status = useNodeResource(fetchLnStatus, 15000)
  const stores = useNodeResource(fetchLnStores, 30000)
  const [pending, setPending] = useState<string | null>(null)
  const [passphraseFor, setPassphraseFor] = useState<
    { kind: 'open'; peer: string; sats: number } | { kind: 'close'; channel: LnChannel } | null
  >(null)
  const toggle = useSubmit()
  const ln = status.data
  async function accept(seller: string, on: boolean, acknowledgeLsp = false) {
    await toggle.run(async () => {
      stores.update(await setStoreAcceptsLightning(seller, on, acknowledgeLsp))
      setPending(null)
      await status.refresh()
    }, 'The store could not be signed again.')
  }
  function choose(seller: string, on: boolean, list: LnStores) {
    if (on && list.lspNotice && !list.lspAcknowledged) setPending(seller)
    else void accept(seller, on)
  }
  return (
    <div className='ob-settings-page'>
      <header className='ob-settings-heading'>
        <h1 className='ob-rail'><RailMark rail='lightning' /> Lightning</h1>
        <p>Take Lightning payments for direct orders. Your node issues one invoice per order. Escrow stays on-chain.</p>
      </header>
      {(status.error || stores.error) && <p className='ob-form-error' role='alert'>{status.error || stores.error}</p>}
      {!ln ? (
        <p className='ob-page-intro' role='status'>{status.loading ? 'Checking Lightning…' : 'Lightning status unavailable.'}</p>
      ) : ln.blocked ? (
        <section className='ob-settings-card'>
          <h2>Lightning is off on mainnet</h2>
          <p>Lightning connections do not run over Tor yet, so they would reveal your node's IP address to peers, the LSP, and chain and gossip servers. Mainnet Lightning stays off until they do. Use on-chain bitcoin, or try Lightning on testnet4.</p>
        </section>
      ) : !ln.enabled ? (
        <section className='ob-settings-card'>
          <h2>Lightning is off on this node</h2>
          <p>Start the node with <code>ob-node serve --lightning</code>. On testnet4 it runs on Mutinynet, a test signet, through the public Mutinynet LSP. With <code>--ln-network testnet4</code> or <code>regtest</code>, name an LSP or turn it off and open channels yourself.</p>
        </section>
      ) : (
        <>
          <NodeCard ln={ln} />
          {stores.data && (
            <section className='ob-settings-card' aria-labelledby='ob-ln-stores'>
              <header className='ob-settings-card-heading'>
                <h2 id='ob-ln-stores'>Stores that take Lightning</h2>
                <p>Turning Lightning on signs your store record again so buyers see it at checkout.</p>
              </header>
              {stores.data.stores.map(store => (
                <div className='ob-setting-row' key={store.seller}>
                  <label htmlFor={`ob-ln-${store.seller}`}>
                    <strong>{store.seller}</strong>
                    <span>{store.own ? 'Your store' : 'Demo store on this node'}</span>
                  </label>
                  <span className='ob-setting-switch'>
                    <input
                      id={`ob-ln-${store.seller}`}
                      type='checkbox'
                      role='switch'
                      checked={store.acceptsLightning}
                      disabled={toggle.busy || !ln.running}
                      onChange={e => choose(store.seller, e.target.checked, stores.data!)}
                    />
                    <span />
                  </span>
                </div>
              ))}
              <FormError message={toggle.error} />
            </section>
          )}
          <ChannelsCard ln={ln} onOpen={(peer, sats) => setPassphraseFor({ kind: 'open', peer, sats })} onClose={channel => setPassphraseFor({ kind: 'close', channel })} />
          <section className='ob-settings-card' aria-labelledby='ob-ln-reveals'>
            <h2 id='ob-ln-reveals'>Who sees what</h2>
            <ul className='ob-ln-reveals'>{ln.reveals.map(line => <li key={line}>{line}</li>)}</ul>
            <p className='ob-settings-footnote'>An invoice shows buyers your Lightning node ID and the LSP, not your IP address.</p>
          </section>
        </>
      )}
      {pending && stores.data?.lspNotice && (
        <Dialog label='How the LSP sees your payments' onCancel={() => setPending(null)}>
          <div className='ob-spend-confirm'>
            <h2>How the LSP sees your payments</h2>
            <p className='ob-page-intro'>{stores.data.lspNotice}</p>
            {ln?.lsp && <CopyAddress value={`${ln.lsp.nodeId}@${ln.lsp.address}`} label='Copy LSP connection'/>}
            <FormError message={toggle.error} />
            <div className='ob-form-actions'>
              <button className='ob-secondary' type='button' onClick={() => setPending(null)}>Cancel</button>
              <button className='ob-primary' disabled={toggle.busy} onClick={() => void accept(pending, true, true)}>
                {toggle.busy ? 'Signing…' : 'I understand, take Lightning'}
              </button>
            </div>
          </div>
        </Dialog>
      )}
      {passphraseFor && (
        <ChannelPassphrase
          title={passphraseFor.kind === 'open' ? `Open a ${passphraseFor.sats.toLocaleString()}-sat channel` : 'Close this channel'}
          onCancel={() => setPassphraseFor(null)}
          onConfirm={async passphrase => {
            if (passphraseFor.kind === 'open') await openLnChannel(passphraseFor.peer, passphraseFor.sats, passphrase)
            else await closeLnChannel(passphraseFor.channel.id, passphrase)
            setPassphraseFor(null)
            await status.refresh()
          }}
        />
      )}
    </div>
  )
}

function NodeCard({ ln }: { ln: LnStatus }) {
  return (
    <section className='ob-settings-card' aria-labelledby='ob-ln-node'>
      <header className='ob-settings-card-heading'>
        <h2 id='ob-ln-node'>Your Lightning node</h2>
        <p>{ln.network ? lnNetworkLabel(ln.network) : ''}</p>
      </header>
      <dl className='ob-spend-terms'>
        <dt>State</dt>
        <dd>
          <span className={`ob-status-dot ${ln.running ? '' : ln.error ? 'is-offline' : 'is-loading'}`} aria-hidden='true' />{' '}
          {ln.running ? 'Running' : ln.keys === 'unlock-to-create' ? 'Unlock the wallet once to create its Lightning keys' : 'Starting'}
        </dd>
        <dt>LSP</dt>
        <dd>{ln.lsp ? <><CopyAddress value={`${ln.lsp.nodeId}@${ln.lsp.address}`} label='Copy LSP connection'/><span className='ob-meta'>{ln.lsp.connected ? 'Connected' : 'Not connected'}</span></> : 'None. Open channels yourself.'}</dd>
        {ln.balances && (
          <>
            <dt>Can receive</dt>
            <dd>{ln.balances.inboundSats.toLocaleString()} sats in open channels{ln.lsp ? '; the LSP opens a channel for larger payments' : ''}</dd>
          </>
        )}
        {ln.nodeId && (
          <>
            <dt>Node ID</dt>
            <dd><CopyAddress value={ln.nodeId} label='Copy Lightning node ID'/></dd>
          </>
        )}
      </dl>
      {ln.error && <p className='ob-form-error' role='alert'>{ln.error}</p>}
    </section>
  )
}

function ChannelsCard({ ln, onOpen, onClose }: { ln: LnStatus; onOpen: (peer: string, sats: number) => void; onClose: (channel: LnChannel) => void }) {
  const [peer, setPeer] = useState(ln.lsp ? `${ln.lsp.nodeId}@${ln.lsp.address}` : '')
  const [sats, setSats] = useState('100000')
  return (
    <section className='ob-settings-card' aria-labelledby='ob-ln-channels'>
      <header className='ob-settings-card-heading'>
        <h2 id='ob-ln-channels'>Channels</h2>
        <p>{ln.lsp ? 'The LSP opens your first channel when a payment arrives. You can also open one yourself.' : 'With no LSP, open a channel to a well-connected node and ask for inbound liquidity.'}</p>
      </header>
      {!ln.channels.length ? (
        <p className='ob-settings-footnote'>No channels yet.</p>
      ) : (
        ln.channels.map(channel => (
          <div className='ob-setting-row' key={channel.id}>
            <label>
              <strong>{channel.capacitySats.toLocaleString()} sats{channel.peer === ln.lsp?.nodeId ? ' with the LSP' : ''}</strong>
              <span>
                Can send {channel.outboundSats.toLocaleString()} · can receive {channel.inboundSats.toLocaleString()} · {channel.usable ? 'ready' : channel.ready ? 'peer offline' : 'opening'}
              </span>
            </label>
            <button className='ob-text-link' onClick={() => onClose(channel)}>Close</button>
          </div>
        ))
      )}
      <form
        className='ob-ln-open'
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          onOpen(peer.trim(), Number(sats))
        }}
      >
        <label className='ob-field'>Peer<input value={peer} onChange={e => setPeer(e.target.value)} placeholder='NODE_ID@HOST:PORT' required /></label>
        <label className='ob-field'>Sats<input type='number' min={20000} inputMode='numeric' value={sats} onChange={e => setSats(e.target.value)} required /></label>
        <button className='ob-secondary' disabled={!ln.running}>Open channel</button>
      </form>
      <p className='ob-settings-footnote'>Opening spends from the Lightning node's on-chain wallet. Opening and closing ask for your wallet passphrase.</p>
    </section>
  )
}

function ChannelPassphrase({ title, onCancel, onConfirm }: { title: string; onCancel: () => void; onConfirm: (passphrase: string) => Promise<void> }) {
  const [passphrase, setPassphrase] = useState('')
  const { busy, error, run } = useSubmit()
  return (
    <Dialog label={title} onCancel={onCancel}>
      <form
        className='ob-spend-confirm'
        onSubmit={(e: FormEvent) => {
          e.preventDefault()
          void run(() => onConfirm(passphrase), 'The node refused.')
        }}
      >
        <h2>{title}</h2>
        <Passphrase label='Wallet passphrase' value={passphrase} onChange={setPassphrase} autoFocus />
        <FormError message={error} />
        <div className='ob-form-actions'>
          <button className='ob-secondary' type='button' onClick={onCancel}>Cancel</button>
          <button className='ob-primary' disabled={busy || !passphrase}>{busy ? 'Working…' : 'Confirm'}</button>
        </div>
      </form>
    </Dialog>
  )
}
