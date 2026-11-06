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

## Mechanisms

Labels: **tested** = code on main with automated tests; **demo** = code runs, but not in the intended deployment (fixture keys, loopback instead of Tor, in-memory state); **specified** = designed, no code; **future** = direction only.

### Catalog

| Mechanism | Status | Notes |
|---|---|---|
| Signed store and listing records, canonical encoding, BIP 340 signatures | tested | Encode, decode, sign, verify; tamper tests. |
| Listing address = SHA-256 of body | tested | Order letters commit to it. |
| Vendor content self-labels (rating, tags) in the signed body | tested | Store v2, listing v5; unlabeled v1/v4 still decode. |
| Third-party labeler sets (`OBLB`) | tested | Signed, sequenced, retractable. Node fetches only from a built-in sample labeler or a loopback URL; no Tor. |
| Client safe mode and keyword filter | tested | Runs on the device only. |
| Catalog served to client | demo | Fixture catalog of 38 listings signed with fixture keys, verified at startup. |
| Replication by every node | specified | No replication code. |
| Publishing cost against spam | specified | No fee design in code. |
| Freshness and completeness | future | No sequence number or signed index. |
| Rejecting street, email, phone in public records | specified | Free-text fields are not filtered. |

### Client, node, and keys

| Mechanism | Status | Notes |
|---|---|---|
| Client talks only to its own node on loopback, with a pairing token | tested | `GET /pair` hands the token to any loopback caller. |
| Node rejects client-named destinations | tested | URL, onion, or peer keys in a request are refused. |
| Desktop app starts its own node | demo | Testnet4. |
| Phone pairs with a desktop or Android node | specified | The client dials only 127.0.0.1. |
| Addresses, orders, seed, wallet, labeler follows encrypted at rest | tested | ChaCha20-Poly1305; `vault.key` (mode 0600) in the same directory. |
| BIP 39 seed with role paths | tested | Spend, store, escrow (`m/1984'/c'/1'/i'`), escrow mail (`m/1984'/c'/2'/0'`), scan. |
| Catalog signed with the seed's store key | not yet | Catalog, BOLT 12 offer, and BIP 352 code use fixture keys. |
| Vendor mailbox key bound to the signed store record | specified | Mailbox keys come from a fixture table; one fixture seller reuses the store scalar. |

### Orders

| Mechanism | Status | Notes |
|---|---|---|
| HPKE sealing of orders, replies, and escrow messages | tested | Separate `info` strings. |
| Mailbox storage without the store secret | tested | |
| Arti onion client and listener | library | Live dial test ignored; not called by the node. |
| Order delivery buyer node to vendor node | specified | The node writes order letters to its own mailbox. |
| Outbox and retry | specified | |
| One letter per seller in a multi-seller checkout | tested | |
| Padding | future | Ciphertext = plaintext + 16 bytes. |
| Mixnet delivery | future | |

### Payments

| Mechanism | Status | Notes |
|---|---|---|
| One payment per seller | tested | One recipient plus change. |
| Fresh per-order vendor address, BIP 21 URI | tested | BDK, BIP 86. |
| Chain watcher | tested | Esplora: mempool.space; testnet4 falls back to blockstream.info; genesis check per server. 1 confirmation on testnet4, 3 on mainnet. |
| Pay from the node | tested | Mainnet needs `--network mainnet` and `"confirm": true`. |
| Coin selection avoiding shared ancestry across one checkout | future | Own unconfirmed change is spendable, so consecutive payments can chain. |
| BOLT 12 offer | encoding only | No Lightning node answers; issuer key is the store key. |
| BIP 352 code | encoding only | No scanning. |
| Payjoin v2 | library only | Checkout does not start a session. |
| Chain queries over Tor / own node / compact filters | specified | |
| Exchange rates | tested | Coinbase over clearnet. |

### Escrow

| Mechanism | Status | Notes |
|---|---|---|
| Two-party protocol (rounds, verifiable shares, refund before funding, claim, moderator record and policy checks, anchors, per-order keys, letters, rulings, ruling store) | tested | 38 unit tests and 5 doctests. |
| Node runs escrow as buyer, vendor, or moderator | demo | Testnet4 only; mainnet refused. Three-node harness tests release, buyer-won dispute, failed broadcast, external funding. |
| Escrow messages between nodes | demo | Sealed letters POSTed to operator-configured loopback URLs (`--escrow-vendor`, `--moderator`); `POST /escrow/mail` takes no token; no Tor, no retry. |
| Durable escrow sessions | not yet | Only the buyer's refund is written to disk; the vendor's claim is in memory. |
| Durable ruling store | not yet | `InMemoryRulingStore` only. |
| Coin locking between funding rounds | not yet | |
| Vendor-initiated dispute in the node | not yet | The crate supports it. |
| Fee bump through the anchor | not yet | No wallet code spends the anchor. |
| Moderator and mailbox keys from real records | not yet | Fixture moderator; anyone can rebuild its secrets. |
| `DisputeEnvelope` without ship-to | specified | The protocol type still embeds the order; the node's dispute path does not use it. |
| Moderator fee on every post-dispute path | specified | Two options documented in the crate. |
| Batched share verification | future | |
| FROST moderator panels | future | |

### Discovery

| Mechanism | Status | Notes |
|---|---|---|
| On-device search | tested (client) | Substring filter. |
| Seeds, pasted links, remembered peers | specified | |
| Eclipse resistance | specified | |
| PIR search | future | |

### Code notes the paper leaves out

- The escrow crate's signed-terms wrapper reuses the magic `OBST`. It is never parsed where a store record is, but it should get its own magic.
- The paper's store record carries the vendor's mailbox key and onion address; `OBST` v2 has neither yet, and no derivation path for the mailbox key is assigned.
- The crate supports per-order keys tweaked by an order secret; the node derives per-order keys from the seed instead.
- Exchange rates come from Coinbase and chain data from public Esplora servers, both over clearnet.
- Presigned escrow spends use a fixed 1,000-sat fee in the node.

## Escrow review

An internal adversarial review read the two-party escrow at `dc7bc42`. Its verdict: the cryptographic core holds (bitwise ElGamal verifiable encryption with OR-proofs and a DLEQ link; no rogue-key or cancellation attack; no signature outside the presigned set; fresh single-use nonces; funding withheld until refund and buyer-wins pre-signature verify; 20,000 fuzzed decoder inputs with no panics). Status at `aa9db48`:

| ID | Finding at `dc7bc42` | Fix at `aa9db48` | Test | Status |
|---|---|---|---|---|
| H1 | Past heights accepted | `h_D >= tip + 1008`, `h_R <= tip + 12960` at funding | `h1_past_or_too_close_heights_are_rejected_at_funding` | fixed |
| H2 | Moderator payout could be non-standard, dust, or the vendor's script | Standard types above dust, distinct scripts, exact moderator record | `h2_moderator_payout_must_be_standard_published_and_distinct`, `h2_moderator_refuses_letters_with_a_different_payout_or_fee` | fixed |
| H3 | Node used single-process escrow | Node runs the two-party API; shim deleted | `escrow_released_to_the_vendor_across_three_nodes`, `escrow_dispute_ruled_for_the_buyer` | fixed |
| M1 | Rulings did not sign the terms | Ruling signs terms digest, output key, outpoint, branch, adaptor point; chain check | `m1_rulings_over_different_terms_are_not_equivocation`, `m1_moderator_checks_the_funding_output_on_chain` | fixed |
| M2 | Stateless moderator could release both branches | Ruling store keyed by (terms digest, outpoint) | `m2_moderator_refuses_a_second_branch` | fixed (store in memory) |
| M3 | Fee bias toward buyer-wins rulings | Two options documented | — | proposal only |
| M4 | Fixed fees, no anchor | 240-sat P2A on refund, claim, disputes; 4 sat/vB floor; version 2, not TRUC | `m4_presigned_spends_carry_a_pay_to_anchor_output` | fixed |
| M5 | Private moderator policy | Policy in the moderator record inside the signed terms | `m5_moderator_policy_is_published_and_checked_at_funding` | fixed |
| M6 | 50 KB shares, costly verification | No re-verify on decrypt; verified-letter cache | `m6_cached_letters_still_get_the_deadline_and_chain_checks` | partly fixed (no batching) |
| L | Clone states, no max fee, release not re-signable, funding RBF, one moderator key | Not Clone; 50,000-sat cap; re-sign; RBF rejected; separate keys | `low_*`, `funding_psbts_that_signal_rbf_are_rejected` | fixed |

Remaining blockers before any mainnet escrow: durable sessions and claims, durable ruling store, real moderator and mailbox keys, authenticated delivery over Tor with retry, coin locking, vendor-initiated disputes, M3, batched verification, and an external cryptographic review.

## Tests

| Commit | node | escrow | pay | protocol | tormail | Total |
|---|---|---|---|---|---|---|
| `b015bff` | 17 | 6 | 11 | 12 | 9 (+1 ignored) | 55 passed, 1 ignored |
| `58e539e` (PR #2 head) | 40 (+1) | 6 | 13 | 12 | 9 (+1) | 80 passed, 2 ignored |
| `dc7bc42` (reviewed escrow) | — | 26 + 3 doc | — | 12 | — | 41 passed |
| `aa9db48` | 46 (+1) | 38 + 5 doc | 13 | 12 | 10 (+1) | 124 passed, 2 ignored |

The two ignored tests need the live network (an Arti onion dial and a real testnet4 payment).
