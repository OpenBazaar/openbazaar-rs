import { checkStoreImage } from "../stores.ts";
import { homeClient } from "./session.ts";
import { imageDataUri } from "./imageUri.ts";

// Verified image data URIs for this visit, keyed by the hash the listing signed.
const images = new Map<string, Promise<string>>();

async function load(hash: string): Promise<string> {
  const answer = await homeClient().request("GET", `/media/${hash}`);
  if (answer.status !== 200) {
    throw new Error(answer.status === 404 ? "This node does not have that image." : "The image could not be loaded.");
  }
  const image = checkStoreImage(hash, answer.headers["content-type"] ?? "", answer.body);
  if (!image) throw new Error("The image the node sent is not the one the store signed.");
  return imageDataUri(image.contentType, image.bytes);
}

/// `/media/{hash}` through the paired LAN or Tor connection, as a data URI.
export function pairedImageUri(hash: string): Promise<string> {
  let pending = images.get(hash);
  if (!pending) {
    pending = load(hash);
    pending.catch(() => images.delete(hash));
    images.set(hash, pending);
  }
  return pending;
}
