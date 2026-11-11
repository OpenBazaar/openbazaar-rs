import { Platform } from "react-native";
import { x25519 } from "@noble/curves/ed25519.js";
import { randomBytes } from "@noble/hashes/utils.js";

import { createKey, signWith } from "../../modules/ob-device-key";
import { pollPairing, requestPairing, type PairingCode, type PendingPairing } from "../homeNode/pairing.ts";
import { base32, discoveryKeyText } from "../homeNode/protocol.ts";
import { channels, ensureTor, insertDiscoveryKey, log, waitForTor } from "./channels.ts";
import { DEVICE_ALIAS, WATCH_ALIAS, boot } from "./session.ts";
import { savePairing } from "./storage.ts";

export type PairingDraft = {
  pending: PendingPairing;
  keyText: string;
  watchKeyText: string;
  discoverySecret: string;
  hardware: boolean;
};

export type PairStep = "keys" | "tor" | "request";

/** Make this phone's keys and send POST /pair/request. */
export async function beginPairing(code: PairingCode, name: string, onStep: (step: PairStep, detail?: string) => void): Promise<PairingDraft> {
  void ensureTor();
  const local = Boolean(code.nodePublic && code.onion && code.lan.length);
  log(`pairing: started; the code ${local ? "has addresses on the local network, tried first" : "is reached over Tor"}`);
  onStep("keys");
  const device = await createKey(DEVICE_ALIAS, "device");
  const watch = await createKey(WATCH_ALIAS, "watch");
  const discovery = randomBytes(32);
  const keyText = `p256:${device.publicKey}`;
  const watchKeyText = `p256:${watch.publicKey}`;
  if (!(code.nodePublic && code.lan.length)) {
    onStep("tor");
    await waitForTor(180_000, status => onStep("tor", `${Math.round(status.progress * 100)}%`));
  }
  onStep("request");
  // Reaching a new code over Tor can take minutes: show the time passing, and why a try ended.
  const asked = Date.now();
  let tries = 1;
  let ended = "";
  const waited = () => Math.floor((Date.now() - asked) / 1000);
  log("pairing: asking the node");
  const clock = setInterval(() => {
    const secs = waited();
    onStep("request", `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}${ended && ` (try ${tries}; the last one ended: ${ended})`}`);
  }, 1000);
  try {
    const pending = await requestPairing(channels, code, {
      key: keyText,
      watchKey: watchKeyText,
      discoveryKey: discoveryKeyText(x25519.getPublicKey(discovery)),
      name,
      platform: Platform.OS === "ios" ? "ios" : "android",
      sign: text => signWith(DEVICE_ALIAS, "device", text),
    }, {
      retrying: reason => {
        log(`pairing: try ${tries} ended after ${waited()}s: ${reason}; asking again`);
        tries++;
        ended = reason;
      },
    });
    log(`pairing: the node answered after ${waited()}s (try ${tries}, by ${pending.route.via}); waiting for the owner to approve`);
    return { pending, keyText, watchKeyText, discoverySecret: base32(discovery), hardware: device.hardware };
  } catch (err) {
    log(`pairing: no answer to keep after ${waited()}s (try ${tries}): ${err instanceof Error ? err.message : err}`);
    throw err;
  } finally {
    clearInterval(clock);
  }
}

/** Long-poll until the owner approves or rejects on the node, or the code expires. */
export async function awaitApproval(draft: PairingDraft, cancelled: () => boolean): Promise<"approved" | "rejected" | "expired" | "cancelled"> {
  let failed = "";
  while (!cancelled()) {
    let result;
    try {
      result = await pollPairing(channels, draft.pending, draft.keyText);
    } catch (err) {
      if (cancelled()) break;
      const reason = err instanceof Error ? err.message : String(err);
      // One line per reason, not per try.
      if (reason !== failed) log(`pairing: could not ask whether the owner approved: ${reason}; asking again`);
      failed = reason;
      await new Promise(resolve => setTimeout(resolve, 3000));
      continue;
    }
    if (result.state === "pending") continue;
    log(`pairing: ${result.state}`);
    if (result.state !== "approved") return result.state;
    await savePairing({ link: result.link, keyText: draft.keyText, watchKeyText: draft.watchKeyText, discoverySecret: draft.discoverySecret, pushEnabled: false });
    await ensureTor();
    await insertDiscoveryKey(result.link.onion, draft.discoverySecret);
    await boot();
    return "approved";
  }
  log("pairing: cancelled on this phone");
  return "cancelled";
}
