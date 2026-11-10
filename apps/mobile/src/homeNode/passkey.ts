import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";
import { paired } from "../catalog.ts";
import { deviceKey } from "./signing.ts";
import { signIn } from "./transport.ts";

// Passkey sign-in for a node's web address (crates/node/src/remote/passkeys.rs).
//
// A passkey cannot sign every request, so it approves this browser instead:
// the browser makes a device key, the passkey signs the node's challenge
// together with that key, and the node adds the key as a session. A passkey
// is created by a browser that is already signed in, which hands the node
// the credential's id and public key. A new node nobody owns yet takes its
// first passkey from the first browser that opens its address.

export type Passkey = { id: string; name: string; addedAt: number };
export type PasskeyInfo = { enabled: boolean; rpId: string; passkeys: Passkey[] };

const json = { "Content-Type": "application/json" };
const record = (value: unknown) => (value && typeof value === "object" ? (value as Record<string, unknown>) : {});
const hex = (buffer: ArrayBuffer) => bytesToHex(new Uint8Array(buffer));

/** Passkeys need https (or localhost) and are bound to a domain, so not on an onion. */
export function passkeysSupported(): boolean {
  return typeof window !== "undefined" && window.isSecureContext && typeof PublicKeyCredential !== "undefined" && !location.hostname.endsWith(".onion");
}

/** `p256:{compressed point hex}` from a credential's public key (SubjectPublicKeyInfo DER, which ends in the uncompressed point). */
export function passkeyKeyText(spki: Uint8Array): string {
  const point = spki.slice(-65);
  if (point.length !== 65 || point[0] !== 4) throw new Error("This passkey's key type is not supported.");
  return `p256:${bytesToHex(new Uint8Array([2 + (point[64] & 1), ...point.slice(1, 33)]))}`;
}

/** What the passkey signs: the node's challenge and the key of the session it approves. */
export function boundChallenge(challengeHex: string, sessionKey: string): Uint8Array {
  return sha256(utf8ToBytes(`ob/passkey/v1\n${challengeHex}\n${sessionKey}`));
}

function parseInfo(value: unknown): PasskeyInfo {
  const body = record(value);
  const list = Array.isArray(body.passkeys) ? body.passkeys.map(record) : [];
  return {
    enabled: body.enabled === true,
    rpId: typeof body.rpId === "string" ? body.rpId : "",
    passkeys: list.flatMap(item => (typeof item.id === "string" ? [{ id: item.id, name: typeof item.name === "string" ? item.name : "Passkey", addedAt: typeof item.addedAt === "number" ? item.addedAt : 0 }] : [])),
  };
}

export async function fetchPasskeys(): Promise<PasskeyInfo> {
  return parseInfo(await paired("/remote/passkeys"));
}

export async function removePasskey(id: string): Promise<PasskeyInfo> {
  return parseInfo(await paired(`/remote/passkeys/${encodeURIComponent(id)}`, { method: "DELETE" }));
}

/** Ask the device for a new passkey for the node at `rpId`. Returns what the node keeps of it. */
async function newCredential(rpId: string): Promise<{ id: string; key: string }> {
  const credential = (await navigator.credentials.create({
    publicKey: {
      rp: { id: rpId, name: "OpenBazaar node" },
      user: { id: randomBytes(16) as BufferSource, name: `OpenBazaar node (${rpId})`, displayName: "OpenBazaar node" },
      // The node checks no attestation: the browser that sends it the key is signed in, or is the node's first.
      challenge: randomBytes(32) as BufferSource,
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      authenticatorSelection: { residentKey: "required", userVerification: "required" },
    },
  })) as PublicKeyCredential | null;
  const spki = (credential?.response as AuthenticatorAttestationResponse | undefined)?.getPublicKey();
  if (!credential || !spki) throw new Error("No passkey was created.");
  return { id: credential.id, key: passkeyKeyText(new Uint8Array(spki)) };
}

/** Ask the device for a new passkey for this node, and register it. The browser must be signed in. */
export async function createPasskey(): Promise<PasskeyInfo> {
  const info = await fetchPasskeys();
  if (!info.enabled) throw new Error("This node has no web address for a passkey to sign in to.");
  return parseInfo(await paired("/remote/passkeys", { method: "POST", headers: json, body: JSON.stringify(await newCredential(info.rpId)) }));
}

/** Become the owner of a new node nobody owns yet: create its first passkey, and sign this browser in with it. */
export async function claimWithPasskey(): Promise<void> {
  const secret = randomBytes(32);
  // The node's passkey domain is the host it serves this page on.
  const passkey = await newCredential(location.hostname);
  await post("/web/passkey/claim", { credentialId: passkey.id, passkey: passkey.key, key: `bip340:${deviceKey(secret).publicKey}` });
  await signIn(secret, true);
}

async function post(path: string, body: object): Promise<Record<string, unknown>> {
  const response = await fetch(path, { method: "POST", headers: json, body: JSON.stringify(body), cache: "no-store" });
  const answer = record(await response.json().catch(() => ({})));
  if (!response.ok) throw new Error(typeof answer.error === "string" ? answer.error : "The node did not accept the passkey.");
  return answer;
}

/** Sign this browser in with a passkey of the node that served the page. It stays signed in until it signs out. */
export async function signInWithPasskey(): Promise<void> {
  const start = await post("/web/passkey/challenge", {});
  if (typeof start.challenge !== "string" || typeof start.rpId !== "string") throw new Error("The node sent no sign-in challenge.");
  const secret = randomBytes(32);
  const key = `bip340:${deviceKey(secret).publicKey}`;
  const credential = (await navigator.credentials.get({
    publicKey: { challenge: boundChallenge(start.challenge, key) as BufferSource, rpId: start.rpId, userVerification: "required" },
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("No passkey was chosen.");
  const answer = credential.response as AuthenticatorAssertionResponse;
  await post("/web/passkey/signin", {
    challenge: start.challenge,
    key,
    credentialId: credential.id,
    authenticatorData: hex(answer.authenticatorData),
    clientDataJSON: hex(answer.clientDataJSON),
    signature: hex(answer.signature),
  });
  await signIn(secret, true);
}
