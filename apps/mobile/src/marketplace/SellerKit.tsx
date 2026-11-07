import { useEffect, useRef, useState, type ReactNode } from "react";
import { fetchStoreImage } from "../stores";
import { uploadImage } from "../seller";
import { isLocked } from "../walletLock";
import { usePairedNode } from "./NodePayments";
import { UnlockDialog } from "./WalletLock";

// Blob URLs of verified images, by hash, for the life of the page.
const images = new Map<string, Promise<string>>();
function imageUrl(hash: string): Promise<string> {
  let url = images.get(hash);
  if (!url) {
    url = fetchStoreImage(hash).then(image => URL.createObjectURL(new Blob([image.bytes.slice().buffer], { type: image.contentType })));
    url.catch(() => images.delete(hash));
    images.set(hash, url);
  }
  return url;
}

/** An image from the node, shown only if its bytes hash to `hash`. */
export function MediaImage({ hash, alt, className, fallback = null }: { hash: string; alt: string; className?: string; fallback?: ReactNode }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    setSrc(null);
    setFailed(false);
    imageUrl(hash).then(url => { if (active) setSrc(url); }, () => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [hash]);
  if (failed) return <>{fallback}</>;
  return src ? <img className={className} src={src} alt={alt} loading="lazy"/> : <span className={`ob-media-loading ${className ?? ""}`} role="img" aria-label={alt}/>;
}

/** A file input that uploads each chosen image and hands back its hash. */
export function ImageUpload({ label, onUploaded, disabled = false, multiple = false }: { label: string; onUploaded: (hash: string) => void; disabled?: boolean; multiple?: boolean }) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function upload(files: FileList | null) {
    if (!files?.length) return;
    setBusy(true);
    setError("");
    try {
      for (const file of Array.from(files)) onUploaded(await uploadImage(file));
    } catch (e) {
      setError(e instanceof Error ? e.message : "The image could not be uploaded.");
    } finally {
      setBusy(false);
      if (input.current) input.current.value = "";
    }
  }
  return <span className="ob-image-upload">
    <label className={`ob-secondary ${disabled || busy ? "is-disabled" : ""}`}>
      {busy ? "Uploading…" : label}
      <input ref={input} type="file" accept="image/png,image/jpeg,image/webp,image/gif" multiple={multiple} disabled={disabled || busy} aria-label={label} onChange={e => void upload(e.target.files)}/>
    </label>
    {error && <span className="ob-form-error" role="alert">{error}</span>}
  </span>;
}

/**
 * Runs a task that needs the store's signing key. When the node answers that
 * the wallet is locked, asks for the passphrase and runs the task again.
 */
export function useLockedAction(onError: (message: string) => void) {
  const { lock } = usePairedNode();
  const [pending, setPending] = useState<{ task: () => Promise<void> } | null>(null);
  async function run(task: () => Promise<void>) {
    try {
      await task();
    } catch (e) {
      if (isLocked(e)) {
        await lock.refresh();
        setPending({ task });
      } else onError(e instanceof Error ? e.message : "The node could not save that.");
    }
  }
  const dialog = pending && lock.data ? <UnlockDialog status={lock.data} reason="Unlock the wallet so your node can sign your store." onCancel={() => setPending(null)} onUnlocked={status => {
    lock.update(status);
    const { task } = pending;
    setPending(null);
    if (status.state === "unlocked") void run(task);
  }}/> : null;
  return { run, dialog };
}
