import { fetchNetwork, type NetworkInfo } from '../nodeWallet'
import { onionLabel, torIndicator, torSummary } from '../torStatus'
import { useNodeResource } from '../useNodeResource'

const TONE_CLASS = { ok: '', busy: 'is-loading', down: 'is-offline' } as const
/** Poll often while Tor is coming up, rarely once it works. */
const pollEvery = (data: NetworkInfo | null) => (data?.tor?.state === 'connected' ? 15000 : 4000)

/** The sidebar line under "Your node is connected". Nothing without Tor. */
export function TorIndicator({ onOpen }: { onOpen: () => void }) {
  const network = useNodeResource(fetchNetwork, pollEvery)
  const indicator = torIndicator(network.data?.tor)
  if (!indicator) return null
  return <button type='button' className='ob-node ob-tor-status' title={indicator.title} onClick={onOpen}>
    <span className={`ob-status-dot ${TONE_CLASS[indicator.tone]}`} aria-hidden='true'/>
    <div><strong>{indicator.label}</strong></div>
  </button>
}

/** Settings › Tor: the connection, the node's onion services, and what waits on Tor. */
export function TorSettings() {
  const network = useNodeResource(fetchNetwork, pollEvery)
  const tor = network.data?.tor
  const indicator = torIndicator(tor)
  return <div className='ob-settings-page'>
    <header className='ob-settings-heading'>
      <h1>Tor</h1>
      <p>Payments, prices, peers, and your store’s onion addresses all go through Tor, so other computers do not see this one’s IP address.</p>
    </header>
    {network.error && <p className='ob-form-error' role='alert'>{network.error}</p>}
    {tor && <section className='ob-settings-card' aria-labelledby='ob-tor-connection'>
      <header className='ob-settings-card-heading'><h2 id='ob-tor-connection'>Connection</h2><p>{torSummary(tor)}</p></header>
      <dl className='ob-spend-terms'>
        <dt>Connection</dt>
        <dd>{indicator ? <><span className={`ob-status-dot ${TONE_CLASS[indicator.tone]}`} aria-hidden='true'/> {indicator.label}</> : 'Not used'}</dd>
        {tor.mode !== 'none' && <><dt>Bootstrap</dt><dd>{tor.state === 'connected' ? 'Done' : tor.progress != null ? `${tor.progress}%` : 'Not reported'}</dd></>}
        {tor.error && <><dt>Last problem</dt><dd>{tor.error}</dd></>}
      </dl>
    </section>}
    {tor && tor.onionServices.length > 0 && <section className='ob-settings-card' aria-labelledby='ob-tor-onions'>
      <header className='ob-settings-card-heading'><h2 id='ob-tor-onions'>Your onion addresses</h2></header>
      <dl className='ob-spend-terms'>
        {tor.onionServices.map(onion => <OnionRow key={onion.name} name={onion.name} published={onion.published}/>)}
      </dl>
      <p className='ob-settings-footnote'>An address that is not published yet cannot be reached. Publishing usually takes a few minutes and is retried on its own.</p>
    </section>}
    {network.data && network.data.torFeatures.length > 0 && <section className='ob-settings-card' aria-labelledby='ob-tor-features'>
      <header className='ob-settings-card-heading'><h2 id='ob-tor-features'>What uses Tor</h2></header>
      <dl className='ob-spend-terms'>
        {network.data.torFeatures.map(feature => <FeatureRow key={feature.name} name={feature.name} state={feature.state}/>)}
      </dl>
    </section>}
    {tor && tor.mode !== 'none' && <details className='ob-wallet-connection'>
      <summary>Details</summary>
      <dl className='ob-spend-terms'>
        <dt>Tor</dt><dd>{tor.mode === 'builtin' ? 'Built into OpenBazaar' : 'A Tor proxy on this computer'}</dd>
        {network.data?.proxy && <><dt>Proxy</dt><dd><code>{network.data.proxy.replace(/^socks5h:\/\//, '')}</code></dd></>}
        <dt>State</dt><dd>{tor.state}</dd>
      </dl>
    </details>}
  </div>
}

function OnionRow({ name, published }: { name: string; published: boolean }) {
  return <><dt>{onionLabel(name)}</dt><dd><span className={`ob-status-dot ${published ? '' : 'is-loading'}`} aria-hidden='true'/> {published ? 'Published' : 'Not published yet'}</dd></>
}

function FeatureRow({ name, state }: { name: string; state: string }) {
  return <><dt>{name}</dt><dd>{state}</dd></>
}
