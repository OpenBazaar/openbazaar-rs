import { useState, type FormEvent } from 'react'
import { enableMonero, fetchXmrBackend, fetchXmrStores, setStoreAcceptsXmr, type XmrBackend, type XmrStores } from '../xmr'
import { useNodeResource } from '../useNodeResource'
import { RailMark } from './RailMark'
import { FormError, useSubmit } from './WalletLock'

const BACKEND_LABEL: Record<XmrBackend['backend'], string> = {
  none: 'Off',
  'own-node': 'Your own monerod',
  'remote-node': 'A remote node'
}

/** Seller settings: which stores take Monero, and the daemon the node reads blocks from. */
export function MoneroSettings() {
  const backend = useNodeResource(fetchXmrBackend, 15000)
  const stores = useNodeResource(fetchXmrStores, 30000)
  const toggle = useSubmit()
  const [daemonUrl, setDaemonUrl] = useState('http://127.0.0.1:38081')
  const xmr = backend.data
  function submitEnable(e: FormEvent) {
    e.preventDefault()
    void toggle.run(async () => {
      backend.update(await enableMonero(daemonUrl))
      await stores.refresh()
    }, 'Monero could not be enabled.')
  }
  async function accept(seller: string, on: boolean) {
    await toggle.run(async () => {
      stores.update(await setStoreAcceptsXmr(seller, on))
    }, 'The store could not be signed again.')
  }
  return (
    <div className='ob-settings-page'>
      <header className='ob-settings-heading'>
        <h1 className='ob-rail'><RailMark rail='monero' /> Monero</h1>
        <p>Take Monero for direct orders. Each order gets its own subaddress. Escrow stays on bitcoin. Experimental.</p>
        {xmr?.network === 'stagenet' && <p>Monero stagenet is a test network: its coins have no value.</p>}
      </header>
      {(backend.error || stores.error) && <p className='ob-form-error' role='alert'>{backend.error || stores.error}</p>}
      {!xmr ? (
        <p className='ob-page-intro' role='status'>{backend.loading ? 'Checking Monero…' : 'Monero status unavailable.'}</p>
      ) : xmr.backend === 'none' ? (
        <>
        <section className='ob-settings-card'>
          <h2>Monero is off on this node</h2>
          <p>
            Connect a <code>monerod</code> RPC URL to enable Monero on the fly (your own node locally, or a remote node over Tor).
            {xmr.network === 'stagenet' ? ' On testnet4 the node pays on Monero stagenet.' : ''}
          </p>
          <form onSubmit={submitEnable} className='ob-form-stack'>
            <label className='ob-field'>
              <span>Monero daemon URL</span>
              <input value={daemonUrl} onChange={e => setDaemonUrl(e.target.value)} placeholder='http://127.0.0.1:38081' required />
            </label>
            <FormError message={toggle.error} />
            <div className='ob-form-actions'>
              <button type='submit' className='ob-primary' disabled={toggle.busy || !daemonUrl.trim()}>
                {toggle.busy ? 'Enabling…' : 'Enable Monero'}
              </button>
            </div>
          </form>
        </section>
        {stores.data?.stores.some(store => store.problem) && (
          <StoresCard stores={stores.data.stores.filter(store => store.problem)} busy={toggle.busy} error={toggle.error} onAccept={accept} />
        )}
        </>
      ) : (
        <>
          <DaemonCard xmr={xmr} />
          {stores.data && <StoresCard stores={stores.data.stores} busy={toggle.busy} error={toggle.error} onAccept={accept} />}
          <section className='ob-settings-card' aria-labelledby='ob-xmr-reveals'>
            <h2 id='ob-xmr-reveals'>Who sees what</h2>
            <p>{xmr.reveals}</p>
            <p className='ob-settings-footnote'>
              The node finds payments by scanning outputs with its view key, which stays on this node. Buyers see one subaddress per order.
            </p>
          </section>
        </>
      )}
    </div>
  )
}

function StoresCard({ stores, busy, error, onAccept }: { stores: XmrStores['stores']; busy: boolean; error: string; onAccept: (seller: string, on: boolean) => Promise<void> }) {
  return (
    <section className='ob-settings-card' aria-labelledby='ob-xmr-stores'>
      <header className='ob-settings-card-heading'>
        <h2 id='ob-xmr-stores'>Stores that take Monero</h2>
        <p>Turning Monero on signs the store record again so buyers see it at checkout.</p>
      </header>
      {stores.length === 0 && <p className='ob-page-intro'>No store on this node can take Monero yet.</p>}
      {!stores.some(store => store.own) && <p className='ob-page-intro'>Your store shows here once it is open. Open it in Store settings.</p>}
      {stores.map(store => (
        <div className='ob-setting-row' key={store.seller}>
          <label htmlFor={`ob-xmr-${store.seller}`}>
            <strong>{store.seller}</strong>
            <span>{store.own ? 'Your store.' : 'Demo store on this node'}</span>
            {store.problem && <span className='ob-form-error' role='alert'>Monero orders are declined for now. {store.problem}</span>}
          </label>
          <span className='ob-setting-switch'>
            <input id={`ob-xmr-${store.seller}`} type='checkbox' role='switch' checked={store.acceptsXmr} disabled={busy} onChange={e => void onAccept(store.seller, e.target.checked)} />
            <span />
          </span>
        </div>
      ))}
      <FormError message={error} />
      <p className='ob-settings-footnote'>
        A buyer who chooses Monero gets a subaddress and an XMR amount. This node watches the subaddress and marks the sale paid after 10 confirmations.
      </p>
    </section>
  )
}

function DaemonCard({ xmr }: { xmr: XmrBackend }) {
  const behind = xmr.tip != null && xmr.scannedHeight != null ? xmr.tip - xmr.scannedHeight : null
  return (
    <section className='ob-settings-card' aria-labelledby='ob-xmr-daemon'>
      <header className='ob-settings-card-heading'>
        <h2 id='ob-xmr-daemon'>Daemon</h2>
        <p>Monero {xmr.network}</p>
      </header>
      <dl className='ob-spend-terms'>
        <dt>Source</dt>
        <dd>
          {BACKEND_LABEL[xmr.backend]}
          {xmr.proxied ? ' over Tor' : ''}
        </dd>
        {xmr.nodes.length > 0 && (
          <>
            <dt>Nodes</dt>
            <dd><code>{xmr.nodes.join(', ')}</code></dd>
          </>
        )}
        <dt>Scanned</dt>
        <dd>
          <span className={`ob-status-dot ${xmr.error ? 'is-offline' : behind === 0 ? '' : 'is-loading'}`} aria-hidden='true' />{' '}
          {xmr.scannedHeight == null
            ? 'Not yet'
            : `Block ${xmr.scannedHeight.toLocaleString()}${xmr.tip != null ? ` of ${xmr.tip.toLocaleString()}` : ''}`}
        </dd>
      </dl>
      {xmr.error && <p className='ob-form-error' role='alert'>{xmr.error}</p>}
    </section>
  )
}
