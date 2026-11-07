# Implementation status

This file tracks how much of the design in `docs/paper` the reference implementation in this repository covers. The paper describes the network as a design; status, commit references, and review tracking live here instead.

Last checked against `main` at `b1eb18b` (escrow, payments, and transport code unchanged since `aa9db48`).

## History

| Commit | What it added |
|---|---|
| `b015bff` | Signed catalog, sealed orders, loopback node, single-process escrow demonstration |
| `5a4e486` | PR #2: BIP 39 seed and role keys, BDK wallet, testnet4 payments, Esplora chain watcher, mainnet switch |
| `286e084` | PR #1: desktop packaging with a bundled node on a free loopback port |
| `aa9db48` | PR #3 (squashed from `3d5af42`): two-party escrow, verifiable moderator shares, review fixes, node integration, three-node test harness |
| `92b2e2e` | PR #5: content ratings and tags, client safe mode, subscribable labelers |
| `chain-privacy` | BIP 157/158 compact block filters as the default chain backend, own-node mode, public Esplora only as an opt-in fallback |
| `monero-direct` | Monero for direct pay only: `crates/xmr` on monero-oxide, per-store opt-in in the signed store record, fresh subaddress per order, pay from the node wallet with a spend ticket or from an external wallet by `monero:` URI |
| `sec-escrow-durable` | Durable escrow on the node: sealed sessions, ruling store, coin locks, outbox with retry, vendor disputes, CPFP bumps, presigned fee from the chain source, M3 fee on the claim, M6 limits, moderator funding check on compact filters; [review packet](escrow-review-packet.md) |
| `home-node` | Home node: remote management over an Arti onion service with restricted discovery, words/QR pairing with an emoji check and approval on the node, per-device keys and signed replay-protected requests, revocation with address rotation, encrypted LAN channel and setup page, delta sync and batches, opt-in content-free push, desktop client and home mode, systemd/launchd/Docker packaging and release builds; [home-node.md](home-node.md) |
| `sec-identity-letters` | PR #9: seed-derived store keys, store record v3 with a mailbox key, onion, moderators, profile and the signed Monero opt-in, signed letters with replay and sender-pin checks, padded letters and replies, client-side BIP-340 verification, a location/contact text validator, real fixture stores, `/stores` and `/media` |
| `lightning` | Lightning for direct pay only: embedded LDK node in `crates/lightning`, LSPS2 just-in-time channels from an LSP the operator names, per-order BOLT 11 invoices, keys from the seed, sealed state, wallet-lock gating, store opt-in in the rail-flags byte, seller settings and checkout in the web shop; [lightning.md](lightning.md) |

## Mechanisms

Labels: **tested** = code on main with automated tests; **demo** = code runs, but not in the intended deployment (fixture keys, loopback instead of Tor, in-memory state); **specified** = designed, no code; **future** = direction only.

### Catalog

| Mechanism | Status | Notes |
|---|---|---|
| Signed store and listing records, canonical encoding, BIP 340 signatures | tested | Encode, decode, sign, verify; tamper tests. |
| Listing address = SHA-256 of body | tested | Order letters commit to it. |
| Vendor content self-labels (rating, tags) in the signed body | tested | Store v2, listing v5; unlabeled v1/v4 still decode. |
| Store record v3: mailbox key, onion, accepted moderators, public profile | tested | Tagline, about, avatar and banner as SHA-256 hashes, shipping/returns/processing policies, ships-to country codes, created-on date. v1/v2 still decode; one canonical encoding; every field tamper-tested. |
| Store pages and store media for the paired client | tested | `GET /stores`, `GET /stores/{key}`, `GET /media/{hash}` (PNG/JPEG/GIF/WebP, 2 MiB cap, served only if the bytes hash to the name). |
| Client verifies signed records | tested | `@noble/curves` BIP 340 over each store and listing record; headers are built from the signed bytes; a listing that fails, or whose store is not a verified store, is dropped and counted. |
| Third-party labeler sets (`OBLB`) | tested | Signed, sequenced, retractable. Node fetches only from a built-in sample labeler or a loopback URL; no Tor. |
| Client safe mode and keyword filter | tested | Runs on the device only. |
| Catalog served to client | demo | Testnet4 only: three demo stores (Marlow Goods, Hale Paper, North Kitchen), each with its own fixture key and v3 record, signing its own listings (38 in all), plus the node's own store. Mainnet serves only the node's own store. |
| Replication by every node | specified | No replication code. |
| Publishing cost against spam | specified | No fee design in code. |
| Freshness and completeness | future | No sequence number or signed index. |
| Rejecting street, email, phone in public records | tested | Signing refuses store and listing text with an email, phone number, street address, PO box, or coordinates. Heuristic; decoding does not filter. |

### Client, node, and keys

| Mechanism | Status | Notes |
|---|---|---|
| Client talks only to its own node on loopback, with a pairing token | tested | Host must be `127.0.0.1`, `localhost`, or `[::1]` on the node's port (DNS rebinding); any `Origin` other than the desktop webview and `--shop-origin` is refused before routing. No route returns the token: the desktop app reads `pairing.token` and injects it into its own webview (`--no-pair-codes`); a browser shop trades a one-time code printed on the node's terminal at `POST /pair` (39 bits, 5 tries, 5 minutes, one code per 10 s). Another process of the same user can still read the file (A6). |
| Loopback HTTP limits | tested | 8 KiB head, 64 headers, 72 KiB body, exact `Content-Length`, no chunked bodies, 5 s per request; handler panics answer 500; `no-store`, `nosniff`, and `default-src 'none'` CSP on every response; 500 bodies carry no detail. Proptest over the parser and the router. Requests are served one at a time, so a stalled local connection delays others by up to 5 s. |
| Node rejects client-named destinations | tested | URL, onion, or peer keys in a request are refused. |
| Desktop app starts its own node | demo | Testnet4. |
| Remote management from a phone or laptop (home node) | tested | Protocol v2 ([spec](home-node-protocol.md)). `--remote` publishes an Arti onion service with restricted discovery: only paired devices' Tor client keys can find it. Pairing: 80-bit one-time secret as 8 words or a QR (10 min, 4 live, pause after 20 bad proofs in 10 min), reached over an onion derived from the secret or the LAN; 4-emoji check on both screens; the owner approves on the node. Every request is signed (P-256 or BIP 340 over node id, device, time, nonce, method, target with query, body hash; ±120 s; nonces once). Watch keys may only sync. Removal drops the device and its discovery key; `--rotate` moves the node to a new onion and retires the old one when every device has moved (7-day cap). A device cannot reveal the seed, post escrow mail, use a remembered unlock, or reach `/pair` and `/home*`; spends need the passphrase and a spend ticket every time. CI uses loopback; real Tor runs by hand (pairing 16–22 s, first request after approval up to 74 s while the descriptor republishes, then 4–8 s). See [home-node.md](home-node.md). |
| Home node LAN channel and setup page | tested | `--lan`: port 8790, requests sealed to the node key (X25519, ChaCha20-Poly1305), mDNS with a random instance name, a script-free setup page with the QR and approval on first run only. Not available in Docker's bridge network. |
| Delta sync, batches, push | tested | `POST /remote/sync` returns only changed items by etag; `POST /remote/batch` runs up to 16 queued mutations once (24 h idempotency). Push is opt-in per device, content-free, sent over Tor to `--push-gateway`; no gateway is run by the project. |
| Desktop app manages a home node | tested | Its own node holds the device key and its discovery key (`home-node.obv`) and proxies `/home/proxy/*` over the LAN channel or Tor; the shop switches between the local and the home node. Two-node LAN e2e in CI; live Tor by hand. The app can also run as a home node while open (`--remote --lan` on its own node). |
| Phone browser manages a home node | demo | Tor Browser with a node-issued discovery secret and access key; keys are made and used in JS. Not in CI. |
| Addresses, orders, seed, wallet, labeler follows encrypted at rest | tested | ChaCha20-Poly1305. Key in the OS keystore by default (macOS Keychain, Windows Credential Manager, Linux Secret Service); `--vault passphrase` wraps it with Argon2id (64 MiB, 3 passes) in `vault.lock`; `--vault file` keeps `vault.key` for headless runs and tests. An old `vault.key` is moved and deleted on first start in another mode. Startup refuses a group- or world-readable key, lock, or token, and tightens `.obv` files (earlier nodes wrote `addresses.obv` and `orders.obv` with the umask). |
| Wallet lock on the node | tested | Seeds sealed a second time under a random spend key, wrapped with Argon2id (256 MiB, 3 passes) of a wallet passphrase in `wallet.lock`. Starts locked; spends, escrow signing, seed reveal, and lock settings answer `423` while locked or before setup. Receive tier (xpubs, escrow mail key, store mailbox key, letter root) stays open for the watcher, addresses, incoming orders and mail, and signing letters; escrow steps that sign wait for an unlock. Auto-lock 1 to 60 min (15 default) and on sleep; persisted backoff. Each payment needs a 60 s one-time ticket bound to order, amount, fee cap, and recipient. Biometric step-up and OS screen-lock events are not wired. See [wallet-lock.md](wallet-lock.md). |
| BIP 39 seed with role paths | tested | Spend, store, escrow (`m/1984'/c'/1'/i'`), escrow mail (`m/1984'/c'/2'/0'`), store mailbox (`m/1984'/c'/3'/0'`), letter root (`m/1984'/c'/4'/0'`), scan, Monero (`m/1984'/c'/5'/0'`). |
| Own store signed with the seed's store key | tested | Profile from `store-profile.json`; BOLT 12 offer and BIP 352 code from the seed. The store key is spend tier: the last signed record is kept in `store-record.obv` and served while locked, and signed again at the next unlock if its inputs changed. No listing authoring yet, so the own store has no listings. |
| Vendor mailbox key bound to the signed store record | tested | Orders and escrow mail are sealed to the mailbox key in the seller's signed record. Demo store mailbox secrets are public fixtures, testnet4 only. |

### Orders

| Mechanism | Status | Notes |
|---|---|---|
| HPKE sealing of orders, replies, and escrow messages | tested | Separate `info` strings. |
| Sender authentication inside the sealed letter (`OBLT`) | tested | BIP 340 signature by a key for one conversation (one order, or one escrow and role), derived from the letter root; never the buyer's long-term key. Recipient, kind, and message id are signed. |
| Replay and sender-continuity checks on escrow mail | tested | `letters-seen` refuses a repeated message id; `letter-pins` refuses a different sender key for an escrow and role once a letter was handled. Vendors do not yet run order letters through it (no inbound order path). |
| Per-order reply path in the order (`ReplyPath`) | tested | Reply key and drop id derived per order; optional reply onion field; `OBRF` frame for delivery. No transport sends replies. |
| Mailbox storage without the store secret | tested | |
| Arti onion client and listener | library | Arti 0.48 with restricted discovery. Live dial test ignored. The node uses Arti only for the home node's onion services and its client; not for letters. |
| Order delivery buyer node to vendor node | specified | The node writes order letters to its own mailbox. |
| Outbox and retry | specified | |
| One letter per seller in a multi-seller checkout | tested | |
| Padding | tested | Sealed letters are exactly 2 KiB, 8 KiB, or 64 KiB; replies 2 KiB or 8 KiB. Other sizes are refused. |
| Mixnet delivery | future | |

### Payments

| Mechanism | Status | Notes |
|---|---|---|
| One payment per seller | tested | One recipient plus change. |
| Fresh per-order vendor address, BIP 21 URI | tested | BDK, BIP 86. |
| Chain watcher | tested | Wallet sync, order watcher, broadcast, and escrow chain checks all go through one `ChainSource`. Default: BIP 157/158 compact filters (`bip157` 0.8, Kyoto) from DNS-seeded P2P peers; filters are matched on the node and only matching blocks are fetched, each from a random peer; broadcast goes to one random peer. Filter commitments and headers are sealed in `filters/`; a peer that later serves a different filter for a checked block is refused; reorgs move transactions back to unconfirmed. 1 confirmation on testnet4, 3 on mainnet. |
| Own node | tested | `--peer HOST:PORT` uses only that node's filters (bitcoind with `blockfilterindex=1`, `peerblockfilters=1`); `--esplora URL` uses only the named servers. Neither falls back to anything; a test proves no other host is dialed. Bitcoind RPC and Electrum are not supported. |
| Public Esplora | tested | Never by default on mainnet. `--public-fallback` (default on testnet4 only) asks mempool.space/blockstream.info while no filter peer has answered for 2 minutes. With `--proxy`, each Esplora connection uses a fresh SOCKS username, so Tor gives it its own circuit. `GET /network` reports `chainBackend` (`backend`, `privacy`, `reveals`, `peers`, `scannedHeight`, `syncing`, `fallback`, `fallbackActive`) and `publicChainServer`. |
| Pay from the node | tested | Mainnet needs `--network mainnet` and `"confirm": true`. |
| Coin selection avoiding shared ancestry across one checkout | future | Own unconfirmed change is spendable, so consecutive payments can chain. |
| BOLT 12 offer | encoding only | No Lightning node answers; issuer key is the store key. Lightning direct pay uses per-order BOLT 11 invoices instead (below). |
| BIP 352 code | encoding only | No scanning. |
| Payjoin v2 | library only | Checkout does not start a session. |
| Chain queries over Tor | demo | `--proxy` also carries filter peer connections and broadcasts through SOCKS5 (P2P v1 only; peers see a Tor exit). No Arti, no onion peers. Without `--proxy`, filter peers see the IP address and which whole blocks it fetches, not which scripts it follows. |
| Compact filter limits | tested | Filters cannot find a transaction by txid, so the escrow moderator follows the funding script from the refund horizon (`refund_height - max_refund_horizon`) and holds the letter while the filters scan. No mempool view: incoming payments appear once confirmed. Fees come from recent block averages. A newly followed address is rescanned 144 blocks back; an escrow funding script from the height the moderator asks for. First sync starts at a checkpoint (testnet4 159000, mainnet 974000); testnet4 took 7.5 minutes to reach the tip from one peer. |
| Exchange rates | tested | Coinbase. Over clearnet unless `--proxy socks5h://HOST:PORT` (names resolved by the proxy); `--no-rates` makes no request. `GET /network` reports `clearnet` and a `privacyWarning`. XMR comes from Kraken's XMR/BTC ticker (Coinbase does not list XMR) on the same proxy and switch, as integer sats per XMR in `GET /rates` `xmr`. |

### Monero (direct pay only)

| Mechanism | Status | Notes |
|---|---|---|
| Wallet library | tested | `crates/xmr` on monero-oxide (`monero-wallet` 0.2, `monero-daemon-rpc` 0.2; pure Rust, audited by Cypher Stack in 2025). No `monero-wallet-rpc`, no second key store. |
| Keys | tested | `m/1984'/c'/5'/0'` gives a 32-byte secret; spend key `b` = HMAC-SHA512("OpenBazaar Monero spend key v1", secret) reduced mod l; view key `a` = Hs(b) as in Monero wallets. View key and public spend key in `xmr-watch.obv` (receive tier); the spend key is derived from `spend.obv` for one signature. |
| Store opt-in | tested | `acceptsXmr` in the signed store record v3. `PUT /xmr/stores` signs the store's record again (the node's own store needs an unlocked wallet; `423` otherwise); checkout and `GET /catalog` `acceptsXmr` read the signed flag only. `xmr-stores.obv` keeps the demo stores' choice for the next boot. |
| Fresh subaddress per order, status | tested | Seller's subaddress per order, never reused across restarts. Awaiting payment (shows `receivedPiconero`, `remainingPiconero`) → Payment seen (all of it) → Paid at 10 confirmations; overpayment is Paid with `overpaidPiconero`. Works while locked. No mempool view: a payment is seen from its first confirmation. |
| Pay from the node | tested | Quote (amount, destination subaddress, fee cap 150 % of the quoted fee), passphrase step-up to a one-time ticket, `POST /xmr/orders/{id}/pay` (202), signed on the node's Monero thread: ring 16, CLSAG, Bulletproofs+, one payment plus change, never batched across sellers. Mainnet needs `"confirm": true`. |
| External wallet | tested | `monero:` URI with `tx_amount` on the order; the client draws the QR. |
| Escrow | refused | `"rail": "xmr"` with escrow is refused at checkout and at pay. |
| Chain access | tested (mocked) | `--xmr-daemon URL[,URL]`, no default. Loopback or LAN = own monerod, direct; `.onion` needs `--proxy`; any other remote node needs `--proxy` on mainnet. Per-connection SOCKS username (own circuit), no redirects, genesis check, the broadcast goes to a different node when more than one is listed. Only monerod's block, output, fee, and broadcast routes are allowed; light-wallet routes are refused in the transport. `GET /network` `xmrBackend` reports `backend`, `privacy`, `reveals`. Testnet4 nodes use stagenet. |
| Spend detection | tested | Only spends made by this node: without the spend key the receive tier cannot compute key images. Restoring a seed elsewhere and spending there leaves this balance too high. |
| Remote sellers | specified | Order status comes from the seller wallet on this node, as with the fixture sellers. |

### Lightning (direct pay only)

| Mechanism | Status | Notes |
|---|---|---|
| Node | tested | `crates/lightning`: `ldk-node` 0.7.1 (vendored for a wider `rusqlite` range) behind a `LightningRail` trait, with a mock and a regtest harness. Off unless `--lightning`. [lightning.md](lightning.md) |
| Keys and state | tested | `m/1984'/c'/6'/0'`, two HMAC-SHA512 labels for the LDK seed and the state key. LDK records sealed with ChaCha20-Poly1305 in `wallets/node/lightning/sealed`; the secret in `ln-keys.obv` (receive tier). |
| Store opt-in | tested | Bit 1 of the rail-flags byte in store record v3. `PUT /ln/stores` signs the record again; the first opt-in answers `409` with the LSP notice until acknowledged. |
| Per-order invoice, status | tested | BOLT 11, 1 hour, fresh hash. Paid on settlement with `receivedMsat` and `lspFeeMsat`; works while locked. |
| LSP (LSPS2 just-in-time channels) | tested | Regtest with real LDK nodes; on Mutinynet the public C= LSP issued JIT invoices through the node and the UI. No completed live payment: the Mutinynet faucet needs a GitHub login. `--lsp`, `--lsp-token`, `--no-lsp`; fee cap 5 % or 10,000 sats. No OpenBazaar LSP; no mainnet default. |
| Pay from the node | tested | Quote, spend ticket bound to the order, hash, amount, and fee cap, then pay (202). Mainnet needs `"confirm": true`. Channel open, close, and sweep need the passphrase. Invoices this node issued are refused. |
| Networks | partly | Test mode runs on Mutinynet signet (no public testnet4 LSP); testnet4 and regtest need `--lsp` or `--no-lsp`. Mainnet needs `--ln-chain` and an LSP choice; not tested live. |
| Escrow | refused | `"rail": "ln"` with escrow is refused. |
| Tor | future | `ldk-node` has no SOCKS support; peers, the LSP, the chain server, and the gossip server see the IP address. |
| Remote sellers | specified | Invoices come from a seller wallet on this node, as for Monero. |

### Escrow

| Mechanism | Status | Notes |
|---|---|---|
| Two-party protocol (rounds, verifiable shares, refund before funding, claim, moderator record and policy checks, anchors, per-order keys, letters, rulings, ruling store, snapshots) | tested | 44 unit tests and 5 doctests. |
| Node runs escrow as buyer, vendor, or moderator | demo | Testnet4 only; mainnet refused. Three-node harness tests release, buyer- and vendor-won disputes, failed broadcast, external funding, and every node restarted before every round. |
| Escrow messages between nodes | demo | Sealed letters POSTed to operator-configured loopback URLs (`--escrow-vendor`, `--moderator`); `POST /escrow/mail` takes no token but refuses any `Origin`, bodies over 64 KiB, more than 64 letters at once or 4/s sustained, and new letters while 256 wait; it only queues. The drop does not authenticate the poster; the mail loop does: each letter carries a BIP 340 signature by the sender's per-escrow letter key inside the ciphertext, and replayed message ids or a changed sender key for an escrow and role are dropped before any desk reads the letter. The replay guard is checked before a step and records the letter only after the step succeeded, so a letter that waits for an unlock or for the chain view, or whose step a crash cut short, runs again. Outgoing letters go through a sealed outbox (`escrow-outbox/`) with an idempotency key per message, retried from 2 s doubling to 15 min and dropped after 24 h; a 4xx is not retried. No Tor. |
| Durable escrow sessions | tested | Buyer, vendor, and moderator records sealed under the vault key in `escrow/{buyer,vendor,moderator}/`, versioned (`v: 1`). Escrow snapshots hold no secret key and no secret nonce; the per-order key is derived again from the seed, so resuming needs the spend tier (mail waits for an unlock). The vendor's claim is stored before the release nonce is sent. A buyer restarted in round 1 opens again with fresh nonces and the next attempt number; a vendor restarted after funding sends a fresh release nonce. One lock per escrow instead of one per role. |
| Durable ruling store | tested | One sealed file per (terms digest, outpoint), created with a hard link so exactly one branch wins a race or a crash. |
| Coin locking between funding rounds | tested | The funding PSBT's inputs are locked from `Open` (24 h, then the open is aborted and the order goes back to offered) and held from the buyer's signatures until broadcast; every payment and bump skips them. Sealed in `coin-locks.obv`. |
| Vendor-initiated dispute in the node | tested | `POST /escrow/sales/{id}/dispute`; its own mail kind, so it is signed and pinned in the vendor's role. A vendor-won ruling settles on the vendor's node. |
| Fee bump through the anchor | tested | `POST /escrow/bump` for the buyer's refund or outcome and the vendor's claim or outcome: a CPFP child from the node wallet reaches the asked package rate. Needs the passphrase again and a `maxFeeSats` cap, not a spend ticket (a child pays no recipient). |
| Moderator and mailbox keys from real records | partly | Vendor mailbox keys and accepted moderators come from signed store records; the buyer refuses terms naming a moderator the store's record does not list. The moderator itself and its mailbox key are still fixtures. |
| `DisputeEnvelope` without ship-to | specified | The protocol type still embeds the order; the node's dispute path does not use it. |
| Moderator fee on every post-dispute path | tested | Option (a): the vendor claim pays exactly the vendor-wins outputs, moderator fee included. |
| Presigned fee | tested | The buyer's chain-source estimate times the largest spend's vsize, clamped to the policy floor and cap, carried in the request and signed into the terms. |
| Dispute letter limits (M6) | tested | Before the chain lookup and the proofs: 20 letters in a burst, refilled one per 6 s, and 4 distinct letters per escrow per 24 h. |
| Batched share verification | future | The proofs carry challenges, not commitments, so they cannot be batched without a letter format change. |
| FROST moderator panels | future | |

### Discovery

| Mechanism | Status | Notes |
|---|---|---|
| On-device search | tested (client) | Substring filter. |
| Seeds, pasted links, remembered peers | specified | |
| Eclipse resistance | specified | |
| PIR search | future | |

### Code notes the paper leaves out

- The escrow crate's signed-terms wrapper uses the magic `OBTS`; `OBST` is only the store record.
- A buyer's first order letter cannot prove who the buyer is: by design the buyer has no identity to prove. Letters are signed by per-conversation keys instead of HPKE Auth mode, which would need the sender's key in the clear.
- The crate supports per-order keys tweaked by an order secret; the node derives per-order keys from the seed instead.
- Exchange rates come from Coinbase over clearnet unless `--proxy` is set. Chain data comes from compact filters; public Esplora is asked only with `--esplora` or `--public-fallback`. The desktop app has no proxy, peer, or fallback setting yet.
- The paper describes bitcoin payments only. Monero direct pay is an addition of this implementation; escrow stays bitcoin-only.

## Escrow review

An internal adversarial review read the two-party escrow at `dc7bc42`. Its verdict: the cryptographic core holds (bitwise ElGamal verifiable encryption with OR-proofs and a DLEQ link; no rogue-key or cancellation attack; no signature outside the presigned set; fresh single-use nonces; funding withheld until refund and buyer-wins pre-signature verify; 20,000 fuzzed decoder inputs with no panics). Status at `aa9db48`, updated on `sec-escrow-durable`:

| ID | Finding at `dc7bc42` | Fix at `aa9db48` | Test | Status |
|---|---|---|---|---|
| H1 | Past heights accepted | `h_D >= tip + 1008`, `h_R <= tip + 12960` at funding | `h1_past_or_too_close_heights_are_rejected_at_funding` | fixed |
| H2 | Moderator payout could be non-standard, dust, or the vendor's script | Standard types above dust, distinct scripts, exact moderator record | `h2_moderator_payout_must_be_standard_published_and_distinct`, `h2_moderator_refuses_letters_with_a_different_payout_or_fee` | fixed |
| H3 | Node used single-process escrow | Node runs the two-party API; shim deleted | `escrow_released_to_the_vendor_across_three_nodes`, `escrow_dispute_ruled_for_the_buyer` | fixed |
| M1 | Rulings did not sign the terms | Ruling signs terms digest, output key, outpoint, branch, adaptor point; chain check | `m1_rulings_over_different_terms_are_not_equivocation`, `m1_moderator_checks_the_funding_output_on_chain` | fixed |
| M2 | Stateless moderator could release both branches | Ruling store keyed by (terms digest, outpoint) | `m2_moderator_refuses_a_second_branch`, `the_vendor_disputes_and_a_vendor_ruling_settles_for_it`, `a_dispute_and_its_ruling_survive_moderator_and_buyer_restarts` | fixed (durable store) |
| M3 | Fee bias toward buyer-wins rulings | Option (a): the claim pays the moderator fee | `m3_the_vendor_claim_pays_the_moderator_fee_like_a_vendor_ruling` | fixed |
| M4 | Fixed fees, no anchor | 240-sat P2A on refund, claim, disputes; 4 sat/vB floor; version 2, not TRUC | `m4_presigned_spends_carry_a_pay_to_anchor_output` | fixed |
| M5 | Private moderator policy | Policy in the moderator record inside the signed terms | `m5_moderator_policy_is_published_and_checked_at_funding` | fixed |
| M6 | 50 KB shares, costly verification | No re-verify on decrypt; verified-letter cache; rate limits before the chain lookup and proofs | `m6_cached_letters_still_get_the_deadline_and_chain_checks`, `m6_the_gate_runs_before_proofs_and_only_for_unverified_letters`, `m6_letters_past_the_per_escrow_limit_are_refused_before_review` | fixed (no batching) |
| L | Clone states, no max fee, release not re-signable, funding RBF, one moderator key | Not Clone; 50,000-sat cap; re-sign; RBF rejected; separate keys | `low_*`, `funding_psbts_that_signal_rbf_are_rejected` | fixed |

Remaining blockers before any mainnet escrow: an external cryptographic review ([packet](escrow-review-packet.md)), a real moderator with its own keys and mailbox, delivery over Tor, and TRUC (v3) presigned spends so a CPFP child cannot be pinned.

## Tests

| Commit | node | escrow | pay | protocol | tormail | Total |
|---|---|---|---|---|---|---|
| `b015bff` | 17 | 6 | 11 | 12 | 9 (+1 ignored) | 55 passed, 1 ignored |
| `58e539e` (PR #2 head) | 40 (+1) | 6 | 13 | 12 | 9 (+1) | 80 passed, 2 ignored |
| `dc7bc42` (reviewed escrow) | — | 26 + 3 doc | — | 12 | — | 41 passed |
| `aa9db48` | 46 (+1) | 38 + 5 doc | 13 | 12 | 10 (+1) | 124 passed, 2 ignored |
| `chain-privacy` | 112 (+3) | 38 + 5 doc | 13 | 33 | 10 (+1) | 211 passed, 4 ignored |
| `monero-direct` | 122 (+4) | 38 + 5 doc | 13 | 33 | 10 (+1) | 239 passed (xmr 10 + 8), 5 ignored |
| `sec-identity-letters` | 135 (+4) | 38 + 5 doc | 13 | 42 | 16 (+1) | 267 passed (xmr 10 + 8), 5 ignored; web 52 unit, 15 Playwright |
| `sec-escrow-durable` | 163 (+4) | 44 + 5 doc | 13 | 42 | 16 (+1) | 301 passed (xmr 10 + 8), 5 ignored; web unchanged |
| `home-node` | 180 (+4) | 44 + 5 doc | 13 | 42 | 16 (+1) | 318 passed (xmr 10 + 8), 5 ignored; web 61 unit |

The ignored tests need the live network: an Arti onion dial, a real testnet4 payment and send, a testnet4 compact filter sync, and a stagenet Monero payment (`live_stagenet`).
