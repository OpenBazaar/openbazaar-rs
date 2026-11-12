import { fetchOwnAddress } from "../storeAddress";
import { useNodeResource } from "../useNodeResource";
import { CopyPaymentValue } from "./NodePayments";

/** The address a seller hands to a buyer, and the one that only works on this computer. */
export function StoreAddress() {
  const resource = useNodeResource(fetchOwnAddress, 15000, true, address => !!address.onion);
  const address = resource.data;
  if (!address) return resource.error ? <p className="ob-form-error" role="alert">{resource.error}</p> : <p className="ob-meta" role="status">Loading your store address…</p>;
  return <>
    {address.onion ? <>
      <CopyPaymentValue label="Store address" value={address.onion}/>
      <p className="ob-meta">Hand this address to a buyer. They paste it under Settings, Followed stores. Their node keeps a signed copy of your store, so they still see your listings while you are offline.</p>
    </> : <>
      <h3>Store address</h3>
      <p className="ob-meta">{address.onTor ? "Your node is still starting on Tor. Your store address shows here once it is published, usually within a few minutes." : "This node is not on Tor, so it has no store address a buyer elsewhere can use. Start it with --remote to get one."}</p>
    </>}
    {address.local && <>
      <CopyPaymentValue label="Address on this computer" value={address.local}/>
      <p className="ob-meta">Only a node on this same computer can follow your store at this address.</p>
    </>}
  </>;
}
