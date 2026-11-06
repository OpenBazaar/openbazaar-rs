import { NodeApiError, paired, parseNetworkName, type NetworkName } from './catalog.ts'

export type LockState = 'setup' | 'locked' | 'unlocked'
export type LockStatus = {
  state: LockState
  autoLockMinutes: number
  autoLockRange: [number, number]
  lockInSecs: number | null
  failedAttempts: number
  retryAfterSecs: number
  rememberDevice: boolean
  rememberAvailable: boolean
  backupConfirmed: boolean
  minPassphraseChars: number
}
/** The payment terms exactly as the node will enforce them. */
export type SpendTerms = {
  orderId: string
  network: NetworkName
  amountSats: number
  feeCapSats: number
  address: string
  feeRateSatVb: number
  expiresInSecs: number
}
export type Quote = SpendTerms & { quote: string }
export type Ticket = SpendTerms & { ticket: string }

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The node returned an unreadable wallet lock response.')
  return value as Record<string, unknown>
}
function count(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('The node returned an invalid wallet lock value.')
  return value
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value)
    throw new Error('The node returned an incomplete payment quote.')
  return value
}
export function parseLockStatus(value: unknown): LockStatus {
  const r = record(value)
  if (r.state !== 'setup' && r.state !== 'locked' && r.state !== 'unlocked')
    throw new Error('The node returned an unknown wallet lock state.')
  const range = Array.isArray(r.autoLockRange) ? r.autoLockRange : [1, 60]
  return {
    state: r.state,
    autoLockMinutes: count(r.autoLockMinutes),
    autoLockRange: [count(range[0]), count(range[1])],
    lockInSecs: r.lockInSecs == null ? null : count(r.lockInSecs),
    failedAttempts: count(r.failedAttempts ?? 0),
    retryAfterSecs: count(r.retryAfterSecs ?? 0),
    rememberDevice: r.rememberDevice === true,
    rememberAvailable: r.rememberAvailable === true,
    backupConfirmed: r.backupConfirmed === true,
    minPassphraseChars: count(r.minPassphraseChars ?? 8)
  }
}
export function parseTerms(value: unknown): SpendTerms {
  const r = record(value)
  const network = parseNetworkName(r.network)
  if (!network) throw new Error('The node quoted an unknown bitcoin network.')
  if (typeof r.feeRateSatVb !== 'number' || !(r.feeRateSatVb > 0))
    throw new Error('The node quoted an invalid fee rate.')
  return {
    orderId: text(r.orderId),
    network,
    amountSats: count(r.amountSats),
    feeCapSats: count(r.feeCapSats),
    address: text(r.address),
    feeRateSatVb: r.feeRateSatVb,
    expiresInSecs: count(r.expiresInSecs)
  }
}
function post(path: string, body?: object, method = 'POST') {
  return paired(path, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body ?? {})
  })
}

/** `true` when the node refused because the wallet is locked or not set up. */
export function isLocked(e: unknown): boolean {
  return e instanceof NodeApiError && e.status === 423
}
export async function fetchLock(): Promise<LockStatus> {
  return parseLockStatus(await paired('/lock'))
}
export async function unlockWallet(passphrase: string): Promise<LockStatus> {
  return parseLockStatus(await post('/unlock', { passphrase }))
}
export async function unlockRemembered(): Promise<LockStatus> {
  return parseLockStatus(await post('/unlock', { remembered: true }))
}
export async function lockWallet(): Promise<LockStatus> {
  return parseLockStatus(await post('/lock'))
}
export async function reportActivity(): Promise<LockStatus> {
  return parseLockStatus(await post('/lock/activity'))
}
export async function setupWallet(passphrase: string): Promise<{
  phrase: string[]
  confirmWords: number[]
  lock: LockStatus
}> {
  const r = record(await post('/setup', { passphrase }))
  const phrase = text(r.phrase).split(' ')
  const confirmWords = Array.isArray(r.confirmWords)
    ? r.confirmWords.map(count)
    : []
  return { phrase, confirmWords, lock: parseLockStatus(r.lock) }
}
export async function confirmBackup(words: string[]): Promise<LockStatus> {
  return parseLockStatus(await post('/setup/confirm', { words }))
}
/** The backup phrase, and the word positions still to confirm (if any). */
export async function revealPhrase(
  passphrase: string
): Promise<{ phrase: string[]; confirmWords: number[] }> {
  const r = record(await post('/wallet/phrase', { passphrase }))
  return {
    phrase: text(r.phrase).split(' '),
    confirmWords: Array.isArray(r.confirmWords)
      ? r.confirmWords.map(count)
      : []
  }
}
/** A message for a failed passphrase check, with the wait if there is one. */
export function passphraseError(e: unknown, fallback: string): string {
  if (!(e instanceof Error)) return fallback
  if (
    e instanceof NodeApiError &&
    e.code === 'wrong_passphrase' &&
    e.retryAfterSecs
  )
    return `${e.message} Wait ${e.retryAfterSecs} seconds before the next try.`
  return e.message
}
export async function updateLockSettings(settings: {
  passphrase: string
  autoLockMinutes?: number
  rememberDevice?: boolean
}): Promise<LockStatus> {
  return parseLockStatus(await post('/lock/settings', settings, 'PUT'))
}
export async function quotePayment(id: string): Promise<Quote> {
  const r = record(await post(`/orders/${encodeURIComponent(id)}/pay/quote`))
  return { ...parseTerms(r), quote: text(r.quote) }
}
export async function issueTicket(
  quote: string,
  passphrase: string
): Promise<Ticket> {
  const r = record(await post('/spend-tickets', { quote, passphrase }))
  return { ...parseTerms(r), ticket: text(r.ticket) }
}
