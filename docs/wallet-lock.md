# Wallet lock

The node, not the client, decides whether money can move. Anyone who can sit
at an unlocked OS account can reach the node's loopback port, and the desktop
app hands its webview the pairing token, so the token cannot be what guards a
spend. Spending, signing, and revealing the seed need the spend key, which
exists only in `wallet.lock` (wrapped under the user's wallet passphrase) and
in memory while the wallet is unlocked. Each spend also needs a one-time
ticket that the user confirmed with the passphrase a moment earlier.

Code: `crates/node/src/wallet_lock.rs` (key, sessions, auto-lock, backoff),
`wallet_keys.rs` (tiers and migration), `spend_tickets.rs`,
`wallet_session.rs`, `lock_http.rs`. Tests: `lock_tests.rs`,
`lock_receive_tests.rs`.

## Threat model

Protects against:

- **Casual physical access.** Someone opens the app on your machine while
  you are away. The wallet is locked (it starts locked and locks itself after
  idle time), so they can browse but cannot pay, release or dispute escrow,
  rule a dispute, read the backup phrase, or change lock settings.
- **Another person at your unlocked OS account after auto-lock.** Same as
  above once the auto-lock time has passed or the machine slept. Even while
  the wallet is unlocked, every payment needs the passphrase again, for the
  exact order, amount, fee cap, and address the node shows.
- **A stolen locked laptop.** Data at rest is sealed under the vault key (the
  OS keystore, from the local-surface PR). The seeds are sealed a second time
  under the spend key. Pulling the disk gives an attacker `wallet.lock`, which
  needs an offline Argon2id search at 256 MiB per guess.
- **Guessing at the unlock prompt.** Backoff survives restarts.

Does not protect against:

- **Malware running as the same OS user while the wallet is unlocked.** It can
  read process memory, read the pairing token, and drive the API between your
  unlock and the auto-lock. It still needs the passphrase for a ticket, but it
  can wait for you to type it.
- **Keyloggers**, screen capture, or a modified client that shows you one
  amount and asks the node for another (the node only binds the ticket to the
  terms it showed; a hostile client can still show the user anything).
- **A weak passphrase** against an offline attacker with the data dir. The
  minimum is 8 characters; Argon2id slows a search, it cannot stop one.
- **"Remember on this device"** turns the lock into an OS-account lock: anyone
  at the unlocked account can unlock the wallet. Spends still need the
  passphrase.
- Root, a compromised OS keystore, cold-boot or swap forensics while unlocked.

## Key tiers

Every wallet under `wallets/<name>/` (the node wallet and one per store) has
two files.

| tier | file | sealed with | holds | available while locked |
|---|---|---|---|---|
| receive | `watch.obv` | vault key (OS keystore) | public BIP 86 descriptors (xpubs), escrow mail key, store mailbox key (X25519), letter root, silent payment code, store signing public key | yes |
| receive | `store-record.obv` (data dir) | vault key | the node's own store record as last signed (public bytes) | yes |
| receive | `state.obv` | vault key | BDK changeset (public descriptors, revealed indexes, chain data), next escrow key index | yes |
| spend | `spend.obv` | spend key | the BIP 39 phrase: spend xprv, per-order escrow keys, store signing key, BIP 352 spend key | no |
| — | `wallet.lock` | Argon2id(passphrase) | the spend key | — |
| — | `wallet-lock.obv` | vault key | failure counter, auto-lock minutes, remember flag, backup confirmation, spend key check | yes |

What a locked node still does: watch the chain and update balances, hand out
new receive addresses from the xpubs, take direct (non-escrow) orders, accept
escrow letters into the mailbox, and open the ones that need no signature
(the moderator reviewing a dispute, the buyer's release nonce). Orders and
escrow mail sealed to the store's mailbox key open with the receive tier,
and the letters a locked node sends are signed with per-conversation keys
from the letter root, which never signs a transaction, an escrow step, or a
store record. A letter that waits for an unlock is not yet admitted to the
replay guard, so it is not refused as a repeat when it is read again.

The store signing key stays in the spend tier. Nothing in the store record
needs a new signature while locked: its keys are fixed by the phrase, and the
profile, rating, moderators, and Monero choice change only through routes
that answer `423` while locked (or by editing `store-profile.json`). The node
serves the record it last signed, from `store-record.obv`, and signs it again
at the next unlock if its inputs changed (`GET /stores/{key}` shows
`unpublishedChanges` until then). A `watch.obv` from before the mailbox key
and letter root were added gets them at the next unlock; until then the node
answers `423` to checkout rather than sending an unsigned letter. Letters that
need a signature stay in the mailbox until the next unlock: the vendor's whole
side (terms, refund, release), and the buyer's countersign, funding, and
ruling steps. Payments the user already confirmed but the worker had not
signed fail with "locked" rather than waiting.

The node wallets are BDK watch-only wallets. A signature builds signers from
the phrase for that one operation and drops them.

## Key derivation and storage

- Spend key: 32 random bytes, generated at setup, never derived from the
  passphrase, so changing the passphrase rewraps one file.
- `wallet.lock`: `OBW1 | memory | passes | lanes | salt[16] | nonce[12] |
  ChaCha20-Poly1305(key)`. The header is authenticated, so lowering the cost
  in the file breaks it. Argon2id, **256 MiB, 3 passes, 1 lane**, 32-byte
  output. That takes about a second on a current laptop and is above the
  OWASP minimum (19 MiB, 2 passes) and RFC 9106's second recommended option
  (64 MiB, 3 passes). The vault passphrase mode keeps its 64 MiB.
- The key is held in `Zeroizing` buffers and wiped on lock, on auto-lock, and
  when each copy for a signature is dropped. The `Seed` built for a signature
  is dropped right after it.
- Writes go through a temp file, fsync, rename, and an fsync of the
  directory.

## Sessions, auto-lock, backoff

- `POST /unlock` opens a session; `POST /lock` ends it and drops every ticket.
- Auto-lock: 15 minutes by default, 1 to 60. The timer restarts only on user
  actions (spend routes and `POST /lock/activity`, which the client sends on
  input). Polling, the chain watcher, and the mail loop do not keep the wallet
  open. Idle time is the larger of the monotonic and wall clocks, and a wall
  clock more than 30 s ahead of the monotonic clock between two 1 s ticks
  (the machine slept) locks at once.
- Backoff: one counter for every passphrase check (unlock, ticket, reveal,
  settings, passphrase change), sealed in `wallet-lock.obv` so a restart does
  not reset it. Two free failures, then 2, 4, 8, ... seconds, capped at one
  hour. A success resets it. During the wait even the right passphrase gets
  `429`.

## Spend tickets (step-up)

1. `POST /orders/{id}/pay/quote`: the node syncs the buyer wallet and builds
   the payment it would make. It answers the terms: `orderId`, `amountSats`,
   `address`, `recipientScript`, `feeRateSatVb`, `feeCapSats` (1.5 × the
   quoted fee, rounded up), `network`, and an opaque `quote`.
2. The client shows those values as the node sent them and asks for the
   passphrase. `POST /spend-tickets {quote, passphrase}` checks it (with
   backoff) and answers a one-time `ticket` with the same terms.
3. `POST /orders/{id}/pay {ticket, confirm?}`: the ticket is consumed. It has
   to be for this order, unexpired (60 s), unused, and the order's amount and
   address must still match. The worker checks the built transaction's
   recipient, amount, and fee against the ticket before signing.

The client cannot name an amount, address, or fee in any of these bodies.
Unknown keys are refused. Tickets live in memory only, at most 64 open.

Escrow release, dispute, and the moderator's ruling need an unlocked wallet
but no ticket: they move funds the user already committed to an escrow, to a
destination fixed by the escrow terms.

## Setup, migration, recovery

- A data dir without `wallet.lock` is in setup. Every spend route answers
  `423` with `"code": "setup_required"`, and the client asks for a passphrase.
- `POST /setup {passphrase}` creates the spend key, writes each wallet's
  phrase to `spend.obv`, then `wallet.lock` (the commit point), then
  overwrites and deletes `seed.obv`. It returns the node wallet's phrase this
  one time, with three word positions to confirm through
  `POST /setup/confirm`. A crash before `wallet.lock` leaves `seed.obv`
  intact; the next start deletes the orphaned `spend.obv`.
- Data dirs from before the lock have `seed.obv` (the phrase under the vault
  key) and no `watch.obv`. They open in setup state, `watch.obv` is derived
  from `seed.obv`, and addresses and balances are unchanged. Setup migrates
  them. A store wallet created while locked gets a `seed.obv` and migrates at
  the next unlock.
- `POST /lock/passphrase {current, new}` rewraps the spend key into a new
  `wallet.lock` with an atomic rename. The seeds are not rewritten.
- `POST /wallet/phrase {passphrase}` shows the node wallet's phrase again.
  Store wallets have their own phrases in their `spend.obv`; there is no
  reveal for those yet.
- Forgetting the passphrase means restoring from the phrase. There is no
  import route yet.
- "Remember on this device" (`PUT /lock/settings {rememberDevice: true}`)
  stores a copy of the spend key in the OS keystore (`wallet-<id>` in the
  vault's service), or in `wallet.remember` with `--vault file`. It is not
  offered with `--vault passphrase`, which has no keystore. `POST /unlock
  {remembered: true}` then unlocks; tickets still need the passphrase.

## API

All routes need the loopback guard and the pairing token. Lock errors avoid
`401`/`403` so a client does not drop its pairing.

| route | state | body | answer |
|---|---|---|---|
| `GET /lock` | any | | lock status |
| `POST /setup` | setup | `{passphrase}` | `{phrase, confirmWords, lock}` |
| `POST /setup/confirm` | unlocked | `{words: [..]}` | lock status |
| `POST /unlock` | locked | `{passphrase}` or `{remembered: true}` | lock status |
| `POST /lock` | any | | lock status |
| `POST /lock/activity` | any | | lock status; restarts the auto-lock timer |
| `PUT /lock/settings` | unlocked | `{passphrase, autoLockMinutes?, rememberDevice?}` | lock status |
| `POST /lock/passphrase` | unlocked | `{current, new}` | lock status |
| `POST /wallet/phrase` | unlocked | `{passphrase}` | `{phrase, confirmWords}`; positions until the backup is confirmed |
| `POST /orders/{id}/pay/quote` | unlocked | | `{quote, ...terms}` |
| `POST /spend-tickets` | unlocked | `{quote, passphrase}` | `{ticket, ...terms}` |
| `POST /orders/{id}/pay` | unlocked | `{ticket, confirm?}` | `202`, the order |

Lock status: `state` (`setup`, `locked`, `unlocked`), `autoLockMinutes`,
`autoLockRange`, `lockInSecs`, `failedAttempts`, `retryAfterSecs`,
`rememberDevice`, `rememberAvailable`, `backupConfirmed`,
`minPassphraseChars`.

Also `423` while locked: escrow checkout (`POST /checkout` and `POST /orders`
with an escrow package), `POST /orders/{id}/release`, `POST
/orders/{id}/dispute`, and `POST /moderator/disputes/{id}/rule`.

Errors: `423 {code: "locked" | "setup_required"}`; `400 {code:
"wrong_passphrase", retryAfterSecs}`; `429 {code: "backoff",
retryAfterSecs}`; `409` for an unknown, expired, used, or other-order ticket
and for terms that changed.

## Limits and native work still to do

- **Biometric step-up.** Touch ID (macOS LocalAuthentication), Windows Hello,
  and Android BiometricPrompt are not wired. The plan: the desktop shell keeps
  a device key in the Secure Enclave / TPM behind a biometric prompt, the
  node stores its public half at setup, and `POST /spend-tickets` accepts a
  signature over the quote in place of the passphrase. The node part is a new
  body field; the native part is the Tauri plugin on each OS.
- **OS screen lock.** The node catches sleep through the clock jump. Locking
  on screen lock needs the desktop shell to watch
  `com.apple.screenIsLocked` (macOS), `WTS_SESSION_LOCK` (Windows), or
  logind's `Lock` signal (Linux) and call `POST /lock`.
- Live escrow sessions (the buyer's and vendor's in-memory `BuyerEscrow` and
  `VendorEscrow`) hold their per-order key until the escrow ends, also while
  locked. They are lost on restart today anyway (see STATUS).
- The quote syncs the buyer wallet on the request thread, so it can take as
  long as one Esplora round.
- The fixture moderator's keys are not in either tier; they are testnet
  fixtures.
