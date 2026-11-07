import { Directory, File, Paths } from "expo-file-system";
import * as SecureStore from "expo-secure-store";
import { bytesToHex, hexToBytes, randomBytes } from "@noble/hashes/utils.js";

import type { NodeLink } from "../homeNode/client.ts";
import { openJson, sealJson } from "../homeNode/seal.ts";

// What the phone keeps about its node. Keys and the link are in the OS
// keystore; the replica and queue are sealed files under a keystore key.

export type Pairing = { link: NodeLink; keyText: string; watchKeyText: string | null; discoverySecret: string; pushEnabled: boolean };

const PAIRING = "ob.pairing";
const SEAL_KEY = "ob.sealKey";
const LOCKED = { keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY };
const BACKGROUND = { keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY };

export async function loadPairing(): Promise<Pairing | null> {
  const raw = await SecureStore.getItemAsync(PAIRING, BACKGROUND);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as Pairing;
  } catch {
    return null;
  }
}

export async function savePairing(pairing: Pairing): Promise<void> {
  await SecureStore.setItemAsync(PAIRING, JSON.stringify(pairing), BACKGROUND);
}

async function sealKey(create: boolean): Promise<Uint8Array | null> {
  const stored = await SecureStore.getItemAsync(SEAL_KEY, LOCKED).catch(() => null);
  if (stored && /^[0-9a-f]{64}$/.test(stored)) return hexToBytes(stored);
  if (!create) return null;
  const key = randomBytes(32);
  await SecureStore.setItemAsync(SEAL_KEY, bytesToHex(key), LOCKED);
  return key;
}

function dir(): Directory {
  const home = new Directory(Paths.document, "home-node");
  if (!home.exists) home.create({ intermediates: true, idempotent: true });
  return home;
}

/** A sealed file's value, or null if it is missing or does not open (for example, while the phone is locked). */
export async function readSealed(name: string): Promise<unknown> {
  const file = new File(dir(), name);
  if (!file.exists) return null;
  const key = await sealKey(false);
  return key ? openJson(key, await file.bytes()) : null;
}

export async function writeSealed(name: string, value: unknown): Promise<void> {
  const key = await sealKey(true);
  if (!key) return;
  const file = new File(dir(), name);
  file.write(sealJson(key, value));
}

export async function forgetEverything(): Promise<void> {
  await SecureStore.deleteItemAsync(PAIRING, BACKGROUND);
  await SecureStore.deleteItemAsync(SEAL_KEY, LOCKED);
  const home = new Directory(Paths.document, "home-node");
  if (home.exists) home.delete();
}
