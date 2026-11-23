// A store's signed catalog index (`OBCX`) and the directory's signed admission
// snapshot (`OBAX`). The bytes match crates/protocol/src/catalog_index.rs.
// A record that does not decode, is out of order, or fails its signature is
// null. Listing hashes are SHA-256 of each listing's signed body.
import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

import { bytesToHex, hexToBytes, sha256Hex } from "./records.ts";

const CATALOG_TAG = "ob/catalog-index/v1";
const ADMISSION_TAG = "ob/admission-index/v1";
const MAX_IDS = 4096;

/// Directory key shipped in crates/node/src/seeds.rs (`DOCUMENT_KEY`).
export const DIRECTORY_KEY = "aa8cab2820fa740a098508dea7c1ad0af351c24ff1218871946f77b4976015c0";

function taggedHash(tag: string, body: Uint8Array): Uint8Array {
  const tagHash = sha256(new TextEncoder().encode(tag));
  const joined = new Uint8Array(64 + body.length);
  joined.set(tagHash, 0);
  joined.set(tagHash, 32);
  joined.set(body, 64);
  return sha256(joined);
}

class Reader {
  private at = 0;
  private readonly bytes: Uint8Array;
  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
  }
  get position(): number {
    return this.at;
  }
  take(len: number): Uint8Array {
    if (this.at + len > this.bytes.length) throw new Error("short");
    const out = this.bytes.subarray(this.at, this.at + len);
    this.at += len;
    return out;
  }
  u8(): number {
    return this.take(1)[0];
  }
  u16(): number {
    const b = this.take(2);
    return b[0] | (b[1] << 8);
  }
  u32(): number {
    const b = this.take(4);
    return (b[0] | (b[1] << 8) | (b[2] << 16)) + b[3] * 0x1000000;
  }
  u64(): number {
    const low = this.u32();
    const high = this.u32();
    const value = high * 0x100000000 + low;
    if (!Number.isSafeInteger(value)) throw new Error("u64");
    return value;
  }
  hex(len: number): string {
    return bytesToHex(this.take(len));
  }
}

/// Sorted unique 32-byte ids. An unsorted or repeated id is not an index.
function readIds(reader: Reader): string[] {
  const count = reader.u16();
  if (count > MAX_IDS) throw new Error("count");
  const ids: string[] = [];
  let previous = "";
  for (let i = 0; i < count; i++) {
    const id = reader.hex(32);
    if (previous && id <= previous) throw new Error("order");
    previous = id;
    ids.push(id);
  }
  return ids;
}

function verified(bytes: Uint8Array, tag: string, keyHex: string, read: (reader: Reader) => boolean): string[] | null {
  const key = hexToBytes(keyHex);
  if (!key || key.length !== 32) return null;
  try {
    const reader = new Reader(bytes);
    if (!read(reader)) return null;
    const ids = readIds(reader);
    const body = bytes.subarray(0, reader.position);
    const signature = reader.take(64);
    if (reader.position !== bytes.length) return null;
    if (!schnorr.verify(signature, taggedHash(tag, body), key)) return null;
    return ids;
  } catch {
    return null;
  }
}

/**
 * Listing hashes named by the store's catalog index, when the index is signed
 * by `storeKey` and names this store record. Anything else is no index.
 */
export function verifyCatalogIndex(hex: unknown, storeKey: string, storeRecordHex: string): string[] | null {
  const bytes = typeof hex === "string" ? hexToBytes(hex) : null;
  const record = hexToBytes(storeRecordHex);
  if (!bytes || !record || !/^[0-9a-f]{64}$/.test(storeKey)) return null;
  const digest = sha256Hex(record);
  return verified(bytes, CATALOG_TAG, storeKey, (reader) => {
    if (new TextDecoder().decode(reader.take(4)) !== "OBCX" || reader.u8() !== 1) return false;
    if (reader.hex(32) !== storeKey) return false;
    reader.u64();
    reader.u32();
    return reader.hex(32) === digest;
  });
}

/** Store ids in an admission snapshot signed by `directoryKey`. A bad signature is null. */
export function verifyAdmissionIndex(hex: unknown, directoryKey: string): string[] | null {
  const bytes = typeof hex === "string" ? hexToBytes(hex) : null;
  if (!bytes || !/^[0-9a-f]{64}$/.test(directoryKey)) return null;
  return verified(bytes, ADMISSION_TAG, directoryKey, (reader) => {
    if (new TextDecoder().decode(reader.take(4)) !== "OBAX" || reader.u8() !== 1) return false;
    if (reader.hex(32) !== directoryKey) return false;
    reader.u64();
    reader.u32();
    reader.u64();
    return true;
  });
}
