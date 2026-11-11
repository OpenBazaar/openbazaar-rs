import { requireNativeModule } from "expo-modules-core";

// Embedded Arti (crates/mobile-tor). Bytes cross the bridge as base64.

type Native = {
  start(): Promise<void>;
  status(): string;
  insertDiscoveryKey(onion: string, secretBase32: string): Promise<void>;
  exchange(onion: string, port: number, requestBase64: string, maxBytes: number, timeoutMs: number): Promise<string>;
  log(line: string): void;
};

export type TorStatus = { state: "stopped" | "bootstrapping" | "ready" | "failed"; progress: number; summary: string };

const native = requireNativeModule<Native>("ObTor");

export const startTor = (): Promise<void> => native.start();

export function torStatus(): TorStatus {
  try {
    return JSON.parse(native.status()) as TorStatus;
  } catch {
    return { state: "stopped", progress: 0, summary: "" };
  }
}

export const insertDiscoveryKey = (onion: string, secretBase32: string): Promise<void> => native.insertDiscoveryKey(onion, secretBase32);

/** Write a line to the device log (NSLog, logcat), next to the Tor client's own lines. */
export const deviceLog = (line: string): void => native.log(line);

export const torExchange = (onion: string, port: number, requestBase64: string, maxBytes: number, timeoutMs: number): Promise<string> =>
  native.exchange(onion, port, requestBase64, maxBytes, timeoutMs);
