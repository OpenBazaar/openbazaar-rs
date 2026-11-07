import { xchacha20poly1305 } from "@noble/ciphers/chacha.js";
import { concatBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

// Files the phone keeps about its node (replica, queue) are sealed with
// XChaCha20-Poly1305 under a random key that lives in the OS keystore.
// Layout: "OBS1" ‖ 24-byte nonce ‖ ciphertext.

const MAGIC = utf8ToBytes("OBS1");

export function sealJson(key: Uint8Array, value: unknown): Uint8Array {
  const nonce = randomBytes(24);
  return concatBytes(MAGIC, nonce, xchacha20poly1305(key, nonce, MAGIC).encrypt(utf8ToBytes(JSON.stringify(value))));
}

/** The value, or null if the file is not one of ours or does not open under `key`. */
export function openJson(key: Uint8Array, sealed: Uint8Array): unknown {
  if (sealed.length < 4 + 24 + 16 || MAGIC.some((byte, i) => sealed[i] !== byte)) return null;
  try {
    const plain = xchacha20poly1305(key, sealed.subarray(4, 28), MAGIC).decrypt(sealed.subarray(28));
    return JSON.parse(new TextDecoder().decode(plain));
  } catch {
    return null;
  }
}
