import { sha256 } from "@noble/hashes/sha2.js";
import { wordlist } from "@scure/bip39/wordlists/english.js";

import { PAIRING_SECRET_BYTES } from "./protocol.ts";

// A pairing secret as 8 BIP 39 words: the 80 secret bits, then the first 8
// bits of SHA-256(secret), 11 bits a word, most significant first.

const WORDS = 8;

export function secretToWords(secret: Uint8Array): string[] {
  if (secret.length !== PAIRING_SECRET_BYTES) throw new Error("A pairing secret is 10 bytes.");
  let bits = 0n;
  for (const byte of secret) bits = (bits << 8n) | BigInt(byte);
  bits = (bits << 8n) | BigInt(sha256(secret)[0]);
  const words: string[] = [];
  for (let i = WORDS - 1; i >= 0; i--) words.push(wordlist[Number((bits >> BigInt(i * 11)) & 2047n)]);
  return words;
}

/** The index of a typed word, matched case-insensitively by its first four letters. */
export function wordIndex(typed: string): number {
  const prefix = typed.trim().toLowerCase().slice(0, 4);
  if (!prefix) return -1;
  return wordlist.findIndex(word => (word.length < 4 || prefix.length < 4 ? word === prefix : word.startsWith(prefix)));
}

export type WordsResult = { secret: Uint8Array } | { error: string; badWord?: number };

export function wordsToSecret(text: string): WordsResult {
  const typed = text.trim().split(/[\s,.-]+/).filter(Boolean);
  if (typed.length !== WORDS) return { error: `Type all ${WORDS} words. You typed ${typed.length}.` };
  let bits = 0n;
  for (const [i, word] of typed.entries()) {
    const index = wordIndex(word);
    if (index < 0) return { error: `Word ${i + 1}, “${word}”, is not one the node uses. Check it.`, badWord: i };
    bits = (bits << 11n) | BigInt(index);
  }
  const secret = new Uint8Array(PAIRING_SECRET_BYTES);
  for (let i = 0; i < PAIRING_SECRET_BYTES; i++) secret[i] = Number((bits >> BigInt(8 + (PAIRING_SECRET_BYTES - 1 - i) * 8)) & 255n);
  if (Number(bits & 255n) !== sha256(secret)[0]) return { error: "These words do not add up. Check each one against the node's screen." };
  return { secret };
}
