import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { concatBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

// The LAN channel: POST /lan with E ‖ ChaCha20-Poly1305(Kreq, inner request).
// Only the holder of the node key can read it or answer under Kres.

const LABEL = utf8ToBytes("OBLAN1");
const ZERO_NONCE = new Uint8Array(12);

export type LanSealed = { body: Uint8Array; open(answer: Uint8Array): Uint8Array };

export function lanSeal(nodePublic: Uint8Array, innerRequest: Uint8Array, ephemeral: Uint8Array = randomBytes(32)): LanSealed {
  const E = x25519.getPublicKey(ephemeral);
  const K = hkdf(sha256, x25519.getSharedSecret(ephemeral, nodePublic), LABEL, concatBytes(E, nodePublic), 64);
  const sealed = chacha20poly1305(K.slice(0, 32), ZERO_NONCE, LABEL).encrypt(innerRequest);
  const kres = K.slice(32, 64);
  return {
    body: concatBytes(E, sealed),
    open: answer => chacha20poly1305(kres, ZERO_NONCE, LABEL).decrypt(answer),
  };
}
