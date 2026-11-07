import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

// A browser's device key and signed requests, protocol v2
// (docs/home-node-protocol.md): the key is BIP 340, its key text is
// "bip340:{x-only hex}", the device id is the first 16 hex digits of SHA-256
// of the key text, and each request signs SHA-256 of
// "OBRQ2\n{nodeId}\n{deviceId}\n{time}\n{nonce}\n{METHOD}\n{target}\n{sha256(body) hex}".

export type DeviceKey = { secret: Uint8Array; publicKey: string; id: string };

export function deviceKey(secret: Uint8Array): DeviceKey {
  const publicKey = bytesToHex(schnorr.getPublicKey(secret));
  return { secret, publicKey, id: deviceId(publicKey) };
}

export function deviceId(publicKeyHex: string): string {
  return bytesToHex(sha256(utf8ToBytes(`bip340:${publicKeyHex}`))).slice(0, 16);
}

export function requestPreimage(nodeId: string, device: string, time: number, nonce: string, method: string, target: string, body: Uint8Array): string {
  return `OBRQ2\n${nodeId}\n${device}\n${time}\n${nonce}\n${method}\n${target}\n${bytesToHex(sha256(body))}`;
}

export function sign(key: DeviceKey, preimage: string): string {
  return bytesToHex(schnorr.sign(sha256(utf8ToBytes(preimage)), key.secret, randomBytes(32)));
}

/** The four auth headers for one request. `target` is the path with its query string. */
export function signedHeaders(key: DeviceKey, nodeId: string, time: number, method: string, target: string, body: Uint8Array): Record<string, string> {
  const nonce = bytesToHex(randomBytes(16));
  return {
    "X-Ob-Device": key.id,
    "X-Ob-Time": String(time),
    "X-Ob-Nonce": nonce,
    "X-Ob-Signature": sign(key, requestPreimage(nodeId, key.id, time, nonce, method, target, body)),
  };
}

// An access key is the device secret as `obkey-` and 52 base32 letters. A
// paired device makes one for Tor Browser, which forgets site storage when it
// closes, so it is kept in a password manager and pasted to sign in again.
const ALPHABET = "abcdefghijklmnopqrstuvwxyz234567";
export const ACCESS_KEY_PREFIX = "obkey-";

export function accessKey(secret: Uint8Array): string {
  let bits = 0, value = 0, out = "";
  for (const byte of secret) {
    value = ((value << 8) | byte) & 0x1fff;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out += ALPHABET[(value >>> bits) & 31];
    }
  }
  if (bits > 0) out += ALPHABET[(value << (5 - bits)) & 31];
  return ACCESS_KEY_PREFIX + out;
}

export function parseAccessKey(text: string): Uint8Array | null {
  const body = text.trim().toLowerCase().replace(/[\s-]/g, "").replace(/^obkey/, "");
  if (body.length !== 52 || [...body].some(ch => !ALPHABET.includes(ch))) return null;
  const bytes: number[] = [];
  let bits = 0, value = 0;
  for (const ch of body) {
    value = ((value << 5) | ALPHABET.indexOf(ch)) & 0x1fff;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((value >>> bits) & 255);
    }
  }
  return bytes.length === 32 ? new Uint8Array(bytes) : null;
}
