import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

// Device keys and signed requests for a home node. The format is the node's
// (crates/node/src/remote/auth.rs): the device key is BIP 340 over a 32-byte
// secret, the device id is the first 16 hex digits of SHA-256 of the x-only
// key, and each request signs SHA-256 of
// "OBRQ1\n{nodeId}\n{deviceId}\n{time}\n{nonce}\n{METHOD}\n{path}\n{sha256(body) hex}".

export type DeviceKey = { secret: Uint8Array; publicKey: string; id: string };

export function deviceKey(secret: Uint8Array): DeviceKey {
  const publicKey = bytesToHex(schnorr.getPublicKey(secret));
  return { secret, publicKey, id: deviceId(publicKey) };
}

export function newDeviceKey(): DeviceKey {
  for (;;) {
    const secret = randomBytes(32);
    try {
      return deviceKey(secret);
    } catch {
      // Out of range for secp256k1; vanishingly rare.
    }
  }
}

export function deviceId(publicKeyHex: string): string {
  return bytesToHex(sha256(hexToBytes(publicKeyHex))).slice(0, 16);
}

export function requestDigest(nodeId: string, device: string, time: number, nonce: string, method: string, path: string, body: Uint8Array): Uint8Array {
  const bodyHash = bytesToHex(sha256(body));
  return sha256(utf8ToBytes(`OBRQ1\n${nodeId}\n${device}\n${time}\n${nonce}\n${method}\n${path}\n${bodyHash}`));
}

export function pairDigest(secret: string, publicKey: string, name: string): Uint8Array {
  return sha256(utf8ToBytes(`OBPAIR1\n${secret}\n${publicKey}\n${name}`));
}

export function sign(key: DeviceKey, digest: Uint8Array): string {
  return bytesToHex(schnorr.sign(digest, key.secret, randomBytes(32)));
}

/** The four auth headers for one request. */
export function signedHeaders(key: DeviceKey, nodeId: string, time: number, method: string, path: string, body: Uint8Array): Record<string, string> {
  const nonce = bytesToHex(randomBytes(16));
  return {
    "X-Ob-Device": key.id,
    "X-Ob-Time": String(time),
    "X-Ob-Nonce": nonce,
    "X-Ob-Signature": sign(key, requestDigest(nodeId, key.id, time, nonce, method, path, body)),
  };
}

// An access key is the device secret as `obkey-` and 52 base32 letters. Tor
// Browser forgets site storage when it closes, so a browser device keeps it
// in a password manager and pastes it to sign in again.
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

/** `{secret, address}` from an `obhome:{secret}@{address}` pairing code. */
export function parsePairingCode(text: string): { secret: string; address: string } | null {
  const match = /^obhome:([a-z2-7]{32})@(\S+)$/i.exec(text.trim());
  return match ? { secret: match[1].toLowerCase(), address: match[2].toLowerCase() } : null;
}
