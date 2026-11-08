import { chacha20poly1305, xchacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { concatBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

// The LAN channel: POST /lan with E ‖ ChaCha20-Poly1305(Kreq, inner request).
// The answer is version 2: 0x02 ‖ 24-byte nonce ‖ XChaCha20-Poly1305(Kres).
// A captured request reuses Kres, so a fixed nonce would let a later error
// reveal the first answer. An old raw ciphertext is not decrypted.

const LABEL = utf8ToBytes("OBLAN1");
const RESPONSE_LABEL = utf8ToBytes("OBLAN2");
const ZERO_NONCE = new Uint8Array(12);
const RESPONSE_VERSION = 2;
const RESPONSE_NONCE_LEN = 24;

export type LanSealed = { body: Uint8Array; open(answer: Uint8Array): Uint8Array };

export function lanSealResponse(key: Uint8Array, plaintext: Uint8Array, nonce: Uint8Array): Uint8Array {
  if (nonce.length !== RESPONSE_NONCE_LEN || nonce.every(byte => byte === 0)) {
    throw new Error("A LAN response nonce must be 24 bytes and not all zeros.");
  }
  return concatBytes(new Uint8Array([RESPONSE_VERSION]), nonce, xchacha20poly1305(key, nonce, RESPONSE_LABEL).encrypt(plaintext));
}

export function lanOpenResponse(key: Uint8Array, answer: Uint8Array): Uint8Array {
  if (answer[0] !== RESPONSE_VERSION || answer.length < 1 + RESPONSE_NONCE_LEN + 16) {
    throw new Error("The LAN answer was not sealed by the home node's key.");
  }
  const nonce = answer.slice(1, 1 + RESPONSE_NONCE_LEN);
  if (nonce.every(byte => byte === 0)) throw new Error("The LAN answer was not sealed by the home node's key.");
  return xchacha20poly1305(key, nonce, RESPONSE_LABEL).decrypt(answer.slice(1 + RESPONSE_NONCE_LEN));
}

export function lanSeal(nodePublic: Uint8Array, innerRequest: Uint8Array, ephemeral: Uint8Array = randomBytes(32)): LanSealed {
  const E = x25519.getPublicKey(ephemeral);
  const K = hkdf(sha256, x25519.getSharedSecret(ephemeral, nodePublic), LABEL, concatBytes(E, nodePublic), 64);
  const sealed = chacha20poly1305(K.slice(0, 32), ZERO_NONCE, LABEL).encrypt(innerRequest);
  const kres = K.slice(32, 64);
  return {
    body: concatBytes(E, sealed),
    open: answer => lanOpenResponse(kres, answer),
  };
}
