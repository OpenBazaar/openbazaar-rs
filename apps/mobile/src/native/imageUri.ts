import { base64 } from "@scure/base";

/// A data URI the phone can show. It is not an http address, so it does not
/// depend on the phone's own loopback or on cleartext exceptions.
export function imageDataUri(contentType: string, bytes: Uint8Array): string {
  if (!contentType.startsWith("image/")) throw new Error("The node did not send an image.");
  return `data:${contentType};base64,${base64.encode(bytes)}`;
}
