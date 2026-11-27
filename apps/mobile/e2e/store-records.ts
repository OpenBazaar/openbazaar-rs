import { schnorr } from '@noble/curves/secp256k1.js'
import { sha256 } from '@noble/hashes/sha2.js'
import { bytesToHex, hexToBytes } from '../src/records.ts'

// Version 3 store records signed with a throwaway key, laid out as
// crates/protocol/src/store.rs writes them. The page verifies the signature
// like any other store record, so these stand in for stores other nodes sign.
export type FakeStore = {
  name: string
  region: string
  tagline: string
  avatar?: string
  banner?: string
  lightning?: boolean
  monero?: boolean
  zcash?: boolean
  seed: number
}

const text = new TextEncoder()

function taggedHash(tag: string, body: Uint8Array): Uint8Array {
  const tagHash = sha256(text.encode(tag))
  return sha256(Uint8Array.from([...tagHash, ...tagHash, ...body]))
}

function str(value: string): number[] {
  const bytes = text.encode(value)
  return [bytes.length & 0xff, bytes.length >> 8, ...bytes]
}

function u32(value: number): number[] {
  return [value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff, (value >>> 24) & 0xff]
}

function image(hash?: string): number[] {
  return hash ? [1, ...hexToBytes(hash)!] : [0]
}

export function fakeStore(store: FakeStore): { key: string; record: string } {
  const secret = new Uint8Array(32).fill(store.seed)
  const key = schnorr.getPublicKey(secret)
  const rails = (store.monero ? 1 : 0) | (store.lightning ? 2 : 0) | (store.zcash ? 4 : 0)
  const body = Uint8Array.from([
    ...text.encode('OBST'), 3,
    ...str(store.name), ...str(store.region), ...str(`bolt12-offer:fake:${store.seed}`), ...str(`bip352-code:fake:${store.seed}`),
    ...key, 0,
    ...new Uint8Array(32).fill(store.seed), ...str(''), 0,
    ...str(store.tagline), ...str('About text.'), ...image(store.avatar), ...image(store.banner),
    ...str('Ships in a week.'), ...str('Returns in 14 days.'), ...str('1 to 3 days'),
    1, ...text.encode('US'), ...u32(20260120),
    rails
  ])
  const signature = schnorr.sign(taggedHash('ob/store/v1', body), secret)
  return { key: bytesToHex(key), record: bytesToHex(Uint8Array.from([...body, ...signature])) }
}
