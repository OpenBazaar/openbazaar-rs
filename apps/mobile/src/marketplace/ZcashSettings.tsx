import { fetchZecBackend, fetchZecStores, setStoreAcceptsZec, type ZecBackend } from '../zec'
import { useNodeResource } from '../useNodeResource'
import { RailMark } from './RailMark'
import { FormError, useSubmit } from './WalletLock'

const BACKEND_LABEL: Record<ZecBackend['backend'], string> = {
  none: 'Off',
  'own-node': 'Your own indexer',
  'remote-indexer': 'A remote indexer',
  mock: 'A test chain'
}

/** Seller settings: which stores take Zcash, and the indexer the node reads blocks from. */
export function ZcashSettings() {
  const backend = useNodeResource(fetchZecBackend, 15000)
  const stores = useNodeResource(fetchZecStores, 30000)
  const toggle = useSubmit()
  const zec = backend.data
  async function accept(seller: string, on: boolean) {
    await toggle.run(async () => {
      stores.update(await setStoreAcceptsZec(seller, on))
    }, 'The store could not be signed again.')
  }
  return (
    <div className='ob-settings-page'>
      <header className='ob-settings-heading'>
        <h1 className='ob-rail'><RailMark rail='zcash' /> Zcash</h1>
        <p>Take Zcash for direct orders. Each order gets its own shielded address, and the payment lands in Ironwood. Escrow stays on bitcoin.</p>
      </header>
      {(backend.error || stores.error) && <p className='ob-form-error' role='alert'>{backend.error || stores.error}</p>}
      {!zec ? (
        <p className='ob-page-intro' role='status'>{backend.loading ? 'Checking Zcash…' : 'Zcash status unavailable.'}</p>
      ) : !zec.enabled ? (
        <section className='ob-settings-card'>
          <h2>Zcash is off on this node</h2>
          <p>
            Start the node with an indexer to read Zcash blocks from: your own Zebra and Zaino, <code>ob-node serve --proxy socks5h://127.0.0.1:9050 --zec-indexer http://127.0.0.1:8137</code>, or a public
            one over Tor, <code>ob-node serve --proxy socks5h://127.0.0.1:9050 --zec-indexer https://…</code>. There is no default server.
            Your own indexer is read directly. With it the proxy is only for the ZEC price, which the node asks for over Tor, and without a price no Zcash order can be placed.
            {zec.network === 'testnet' ? ' On testnet4 the node pays on Zcash testnet.' : ''}
          </p>
        </section>
      ) : (
        <>
          <IndexerCard zec={zec} />
          {stores.data && (
            <section className='ob-settings-card' aria-labelledby='ob-zec-stores'>
              <header className='ob-settings-card-heading'>
                <h2 id='ob-zec-stores'>Stores that take Zcash</h2>
                <p>Turning Zcash on signs the store record again so buyers see it at checkout.</p>
              </header>
              {stores.data.stores.length === 0 && <p className='ob-page-intro'>No store on this node can take Zcash yet.</p>}
              {stores.data.stores.map(store => (
                <div className='ob-setting-row' key={store.seller}>
                  <label htmlFor={`ob-zec-${store.seller}`}>
                    <strong>{store.seller}</strong>
                    <span>{store.own ? 'Your store. People still on the first release, v0.1.0-alpha.1, will not see it while Zcash is on, until they update.' : 'Demo store on this node'}</span>
                  </label>
                  <span className='ob-setting-switch'>
                    <input
                      id={`ob-zec-${store.seller}`}
                      type='checkbox'
                      role='switch'
                      checked={store.acceptsZec}
                      disabled={toggle.busy}
                      onChange={e => void accept(store.seller, e.target.checked)}
                    />
                    <span />
                  </span>
                </div>
              ))}
              <FormError message={toggle.error} />
              <p className='ob-settings-footnote'>
                Buyers on other nodes cannot pay your store in Zcash yet. For now the switch only adds Zcash to your store's signed record.
              </p>
            </section>
          )}
          <section className='ob-settings-card' aria-labelledby='ob-zec-reveals'>
            <h2 id='ob-zec-reveals'>Who sees what</h2>
            <p>{zec.reveals}</p>
            <p className='ob-settings-footnote'>
              The node finds payments by trying every new output with its viewing key, which stays on this node. Buyers see one address per order, and addresses are not linkable to each other.
            </p>
          </section>
        </>
      )}
    </div>
  )
}

function IndexerCard({ zec }: { zec: ZecBackend }) {
  const behind = zec.tip != null && zec.scannedHeight != null ? zec.tip - zec.scannedHeight : null
  return (
    <section className='ob-settings-card' aria-labelledby='ob-zec-indexer'>
      <header className='ob-settings-card-heading'>
        <h2 id='ob-zec-indexer'>Indexer</h2>
        <p>Zcash {zec.network}</p>
      </header>
      <dl className='ob-spend-terms'>
        <dt>Source</dt>
        <dd>
          {BACKEND_LABEL[zec.backend]}
          {zec.proxied ? ' over Tor' : ''}
        </dd>
        {zec.servers.length > 0 && (
          <>
            <dt>Servers</dt>
            <dd><code>{zec.servers.join(', ')}</code></dd>
          </>
        )}
        {zec.software && (
          <>
            <dt>Runs</dt>
            <dd>{zec.software}</dd>
          </>
        )}
        <dt>Scanned</dt>
        <dd>
          <span className={`ob-status-dot ${zec.error ? 'is-offline' : behind === 0 ? '' : 'is-loading'}`} aria-hidden='true' />{' '}
          {zec.scannedHeight == null
            ? 'Not yet'
            : `Block ${zec.scannedHeight.toLocaleString()}${zec.tip != null ? ` of ${zec.tip.toLocaleString()}` : ''}`}
        </dd>
      </dl>
      {zec.error && <p className='ob-form-error' role='alert'>{zec.error}</p>}
    </section>
  )
}
