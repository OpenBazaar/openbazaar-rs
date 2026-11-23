import { Dialog } from "./WalletLock";
import { MediaImage } from "./SellerKit";

/** A larger view of one listing photo. The listing still holds at most eight. */
export function PhotoZoom({ hash, title, onClose }: { hash: string; title: string; onClose: () => void }) {
  return (
    <Dialog label={`Larger view of ${title}`} className="ob-photo-dialog" onCancel={onClose}>
      <div className="ob-photo-large">
        <MediaImage hash={hash} alt={title} />
      </div>
      <div className="ob-form-actions">
        <button type="button" className="ob-secondary" onClick={onClose}>Close</button>
      </div>
    </Dialog>
  );
}
