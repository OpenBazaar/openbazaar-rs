import { ed25519 } from "@noble/curves/ed25519.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { sha3_256 } from "@noble/hashes/sha3.js";
import { bytesToHex, concatBytes, hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";
import { base32nopad } from "@scure/base";

// Home node protocol v2 (docs/home-node-protocol.md): ids, the pairing
// transcript, proof and short authentication string, the pairing onion, and
// the request preimage. Every function here has a vector in vectors.test.ts.

export const PAIRING_SECRET_BYTES = 10;
export type Platform = "ios" | "android" | "desktop" | "browser";

export function base32(bytes: Uint8Array): string {
  return base32nopad.encode(bytes).toLowerCase();
}

export function fromBase32(text: string): Uint8Array {
  return base32nopad.decode(text.trim().replace(/=+$/, "").toUpperCase());
}

export function nodeIdOf(nodePublic: Uint8Array): string {
  return bytesToHex(sha256(concatBytes(utf8ToBytes("OBNODE2"), nodePublic)).slice(0, 16));
}

/** `p256:{SEC1 compressed hex}` or `bip340:{x-only hex}`. */
export function deviceIdOf(keyText: string): string {
  return bytesToHex(sha256(utf8ToBytes(keyText)).slice(0, 8));
}

export function discoveryKeyText(publicKey: Uint8Array): string {
  return `descriptor:x25519:${base32(publicKey)}`;
}

export type PairRequestFields = {
  key: string;
  watchKey?: string;
  discoveryKey?: string;
  name: string;
  platform: Platform;
};

export function transcript(fields: PairRequestFields): string {
  return ["OBPAIR2", fields.key, fields.watchKey ?? "", fields.discoveryKey ?? "", fields.name, fields.platform].join("\n");
}

export function pairProof(secret: Uint8Array, text: string): string {
  return bytesToHex(hmac(sha256, secret, utf8ToBytes(text)));
}

// The fixed list from the spec, in order: index = h[i] & 63.
const SAS_TABLE = `🐶 dog 🐱 cat 🦊 fox 🐻 bear 🐼 panda 🐨 koala 🐯 tiger 🦁 lion
🐮 cow 🐷 pig 🐸 frog 🐵 monkey 🐔 chicken 🐧 penguin 🐦 bird 🦉 owl
🦋 butterfly 🐢 turtle 🐙 octopus 🦀 crab 🐳 whale 🐬 dolphin 🐟 fish 🦈 shark
🌵 cactus 🌲 tree 🌻 sunflower 🍄 mushroom 🌙 moon ⭐ star ☀️ sun ⚡ lightning
🔥 fire 💧 droplet 🌈 rainbow ❄️ snowflake 🍎 apple 🍋 lemon 🍌 banana 🍇 grapes
🍓 strawberry 🥕 carrot 🌽 corn 🍕 pizza 🎂 cake ☕ coffee 🚲 bicycle 🚗 car
🚀 rocket ⛵ sailboat 🎈 balloon 🎁 gift 🔑 key 🔔 bell 🎸 guitar 📚 books
⚓ anchor 🧭 compass ⏰ clock 💡 bulb 🔒 lock ✂️ scissors 🧩 puzzle 👑 crown`;

export const SAS_EMOJI: readonly { emoji: string; name: string }[] = (() => {
  const parts = SAS_TABLE.split(/\s+/);
  const list: { emoji: string; name: string }[] = [];
  for (let i = 0; i < parts.length; i += 2) list.push({ emoji: parts[i], name: parts[i + 1] });
  return list;
})();

export function sas(secret: Uint8Array, nodePublic: Uint8Array, text: string): { emoji: string; name: string }[] {
  const h = sha256(concatBytes(utf8ToBytes("OBSAS2\n"), secret, nodePublic, utf8ToBytes(text)));
  return [0, 1, 2, 3].map(i => SAS_EMOJI[h[i] & 63]);
}

export function onionFromPublicKey(publicKey: Uint8Array): string {
  const version = new Uint8Array([3]);
  const checksum = sha3_256(concatBytes(utf8ToBytes(".onion checksum"), publicKey, version)).slice(0, 2);
  return base32(concatBytes(publicKey, checksum, version));
}

/** The onion the node runs for a live pairing code, without ".onion". */
export function pairingOnion(secret: Uint8Array): string {
  const seed = hkdf(sha256, secret, utf8ToBytes("OBPAIR2"), utf8ToBytes("onion"), 32);
  return onionFromPublicKey(ed25519.getPublicKey(seed));
}

export function requestPreimage(nodeId: string, deviceId: string, time: number, nonce: string, method: string, path: string, body: Uint8Array): string {
  return `OBRQ2\n${nodeId}\n${deviceId}\n${time}\n${nonce}\n${method}\n${path}\n${bytesToHex(sha256(body))}`;
}

export type PairingLink = {
  secret: Uint8Array;
  nodePublic: Uint8Array;
  onion: string;
  lan: string[];
};

const ONION = /^[a-z2-7]{56}$/;

/** `openbazaar://pair?v=2&s=…&k=…&o=…&l=…` */
export function parsePairingLink(text: string): PairingLink | null {
  const match = /^openbazaar:\/\/pair\?(.*)$/i.exec(text.trim());
  if (!match) return null;
  const params = new Map<string, string[]>();
  for (const part of match[1].split("&")) {
    const [key, value = ""] = part.split("=");
    params.set(key, [...(params.get(key) ?? []), decodeURIComponent(value)]);
  }
  const one = (key: string) => params.get(key)?.[0] ?? "";
  if (one("v") !== "2") return null;
  const onion = one("o").toLowerCase().replace(/\.onion$/, "");
  const nodeHex = one("k").toLowerCase();
  if (!ONION.test(onion) || !/^[0-9a-f]{64}$/.test(nodeHex)) return null;
  let secret: Uint8Array;
  try {
    secret = fromBase32(one("s"));
  } catch {
    return null;
  }
  if (secret.length !== PAIRING_SECRET_BYTES) return null;
  const lan = (params.get("l") ?? []).filter(address => /^[\w.:[\]-]+:\d{1,5}$/.test(address));
  return { secret, nodePublic: hexToBytes(nodeHex), onion, lan };
}
