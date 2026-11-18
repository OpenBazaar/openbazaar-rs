import { directoryFeeStatus, fetchDirectoryFee } from "../directoryFee";
import { useNodeResource } from "../useNodeResource";
import { CopyAddress } from "./CopyAddress";

/** Where this store's directory fee is paid. The address names this store, so a payment from anywhere lists it. */
export function DirectoryListing() {
  const resource = useNodeResource(fetchDirectoryFee, 15000);
  const fee = resource.data;
  return <section className="ob-store-section" aria-labelledby="ob-directory-listing-title">
    <h2 id="ob-directory-listing-title">Directory listing</h2>
    {!fee ? resource.error ? <p className="ob-form-error" role="alert">{resource.error}</p> : <p role="status">Loading…</p>
      : !fee.available ? <p className="ob-field-hint">{fee.reason}</p>
      : <>
        <p className="ob-field-hint">The directory lists stores that pay {fee.sats.toLocaleString("en-US")} sats every two weeks. Your node pays from its bitcoin wallet when it can. You can also pay this address from any wallet, exchange, or swap service, for example by swapping Monero, Zcash, or Lightning sats into bitcoin sent here. The address belongs to this store, so the payment cannot list anyone else.</p>
        <CopyAddress value={fee.address} label="Copy fee address"/>
        <p role="status">{directoryFeeStatus(fee)}</p>
      </>}
  </section>;
}
