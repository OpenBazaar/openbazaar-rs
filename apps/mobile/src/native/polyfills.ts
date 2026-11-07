import * as Crypto from "expo-crypto";

// @noble draws randomness from globalThis.crypto.getRandomValues.
const scope = globalThis as { crypto?: { getRandomValues?: <T extends ArrayBufferView>(array: T) => T } };
if (!scope.crypto?.getRandomValues) {
  scope.crypto = { ...(scope.crypto ?? {}), getRandomValues: array => Crypto.getRandomValues(array as never) as never };
}
