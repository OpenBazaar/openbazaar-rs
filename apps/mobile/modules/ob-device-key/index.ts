import { requireNativeModule } from "expo-modules-core";
import * as LocalAuthentication from "expo-local-authentication";

// Hardware P-256 keys: Secure Enclave on iOS, StrongBox or TEE on Android.
// `sign` is ECDSA with SHA-256 over the UTF-8 preimage, DER, as hex.

type Native = {
  create(alias: string, userPresence: boolean): Promise<{ publicKey: string; hardware: boolean }>;
  publicKey(alias: string): Promise<string | null>;
  sign(alias: string, preimage: string, userPresence: boolean): Promise<string>;
  delete(alias: string): Promise<void>;
};

const native = requireNativeModule<Native>("ObDeviceKey");

export type KeyKind = "device" | "watch";
const needsOwner = (kind: KeyKind) => kind === "device";

export function createKey(alias: string, kind: KeyKind): Promise<{ publicKey: string; hardware: boolean }> {
  return native.create(alias, needsOwner(kind));
}

export function publicKey(alias: string): Promise<string | null> {
  return native.publicKey(alias);
}

/** On Android a device key needs a recent unlock; ask for one and try once more. */
export async function signWith(alias: string, kind: KeyKind, preimage: string): Promise<string> {
  try {
    return await native.sign(alias, preimage, needsOwner(kind));
  } catch (err) {
    if ((err as { code?: string }).code !== "ERR_AUTH_REQUIRED") throw err;
    const result = await LocalAuthentication.authenticateAsync({ promptMessage: "Unlock your home node" });
    if (!result.success) throw new Error("Unlock this phone to reach your home node.");
    return native.sign(alias, preimage, needsOwner(kind));
  }
}

export function deleteKey(alias: string): Promise<void> {
  return native.delete(alias);
}
