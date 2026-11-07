# Escrow review packet

For a cryptographic auditor reviewing the two-party moderated escrow before any mainnet use. It says what to read, what each part claims, where the claims are enforced, and which questions we most want answered. Implementation status and the earlier internal review are in [STATUS.md](STATUS.md#escrow-review). Mainnet escrow stays refused in the node (`ESCROW_MAINNET_REFUSAL`) until this review is done.

## Scope

| In scope | Path | Lines (approx.) |
|---|---|---|
| Protocol, proofs, transactions | `crates/escrow/src/` | 4,300 |
| Node integration: sessions on disk, mail, moderator desk | `crates/node/src/escrow_*.rs`, `escrow_buyer/`, `escrow_vendor/`, `coin_locks.rs`, `letter_auth.rs` | 4,000 |
| Letter sealing and sender authentication | `crates/tormail/src/seal.rs`, `letter.rs`, `guard.rs` | 900 |

Out of scope: the catalog and store records, payments outside escrow, Monero (escrow refuses XMR), the web and desktop clients.

Dependencies the construction relies on: `musig2` 0.4 (BIP 327 key aggregation, nonces, partial and adaptor signatures), `secp256k1` through `bitcoin` 0.32, `bdk_wallet` 3.2 (funding PSBT and the CPFP child only), `chacha20poly1305` for sealing at rest (`vault.rs`), HPKE (X25519, HKDF-SHA256, ChaCha20-Poly1305) in `tormail`.

## The construction in one page

The funding output is a Taproot key-path-only output whose internal key is the MuSig2 aggregate of the buyer's and the vendor's per-order keys, tweaked with the unspendable BIP 341 tweak. The moderator's key is never aggregated. Every outcome is one 64-byte key-path signature by both parties.

| spend | `nLockTime` | pays | signed |
|---|---|---|---|
| release | 0 | vendor | after funding, by both |
| vendor claim | `V` | vendor, moderator fee, anchor | before funding |
| buyer refund | `R` | buyer, anchor | before funding |
| buyer wins | 0 | buyer, moderator fee, anchor | before funding, as an adaptor signature |
| vendor wins | 0 | vendor, moderator fee, anchor | before funding, as an adaptor signature |

Branch `X` (favoring party `X`) is an adaptor signature for point `T_X + M_X`. `T_X` is derived from `X`'s per-order key and proved with a Schnorr proof of knowledge, so `X` cannot choose it to cancel `M_X`. `M_X` is sampled by the party that loses branch `X` and verifiably encrypted to the moderator's encryption key: 256 exponential-ElGamal bit ciphertexts, one CDS OR-proof per bit that it encrypts 0 or 1, and a Chaum-Pedersen proof that the weighted sum of the bits is `log_G(M_X)`. `X` verifies that proof before signing anything that leads to funding.

To rule, the moderator decrypts `m_X` from a dispute letter written by `X` and sends it back in a signed ruling. `X` then adds `t_X + m_X` to the adaptor signature. The moderator can reveal one half or the other, but cannot redirect funds, change amounts, or sign anything alone. The favored party cannot finish its branch alone, and the losing party cannot finish a branch that pays it.

Rounds (`crates/escrow/src/funding.rs`):

```text
round 0  buyer & vendor   EscrowTerms, both signatures (SignedTerms)
round 1  buyer -> vendor  BuyerOpen: outpoint, nonces, T_b + PoK, enc(M_vendor_wins) + proof
round 2  vendor -> buyer  VendorReply: nonces, T_v + PoK, enc(M_buyer_wins) + proof, partials for refund and buyer-wins
round 3  buyer -> vendor  BuyerSignatures: partials for vendor claim and vendor-wins
release  vendor nonce, then buyer partial
dispute  claimant -> moderator: self-verifying letter; moderator -> claimant: ModeratorRuling
```

The buyer gets its PSBT back to sign the funding only after the refund and the buyer-wins pre-signature verify against the aggregate key and the exact funding outpoint and value.

## Claims to check

1. **No signature outside the presigned set.** The parties sign only the five templates in `tx.rs` (`Templates::build`), all fixed by the signed terms and the outpoint. Check that no message to sign comes from the counterparty.
2. **Nonce safety.** Secret nonces are made with `SecNonce::generate` from fresh randomness plus the key, the aggregate key, and the message (`crypto::fresh_nonce`). They are consumed by value and never stored: no snapshot contains one (`state.rs`, `funding.rs` `VendorAwaitingSignatures::snapshot`). After a restart, round 1 starts again with fresh nonces on both sides, and the release nonce is made again. **Question:** is there any path where a party signs twice with one nonce, including a restored copy? The node keeps one live copy per escrow under one lock, and stores a snapshot before it sends what a step produced.
3. **Rogue keys and cancellation.** BIP 327 key aggregation with both keys fixed in the signed terms. `T_X` comes with a proof of knowledge, and `M_X` with the verifiable encryption.
4. **Verifiable encryption soundness and zero-knowledge** (`vencrypt.rs`, `proofs.rs`). Fiat-Shamir challenges are tagged SHA-256 over length-prefixed inputs, bound to a 32-byte branch context (`Contract::branch_context`: favored party, that branch's sighash, outpoint, terms digest) and, per bit, to the index. **Question:** is binding the whole ciphertext digest into every bit proof enough to stop proofs being mixed across shares, and is the OR-proof's simulation sound with `(c, z)` encoding?
5. **Moderator equivocation is detectable.** A ruling signs the terms digest, the output key, the outpoint, the branch and the adaptor point (`letter.rs`), so two rulings for one escrow are transferable proof (`is_equivocation`). The node's ruling store keeps one branch per escrow across crashes.
6. **Timelocks.** `D < V < R` with policy gaps. At funding, `D >= tip + min_delivery_window` and `R <= tip + max_refund_horizon`. A letter is accepted only up to `D`. **Question:** with presigned fees and a P2A anchor on version 2 transactions, can a vendor claim at `V + 1` and a buyer-wins outcome be pinned against each other?

## What this branch changed

| Change | Where | Review interest |
|---|---|---|
| Snapshots for `BuyerEscrow`, `VendorAwaitingSignatures`, `VendorEscrow`; restore re-verifies the partials, the adaptor pre-signature, and that the secret matches the key in the terms | `crates/escrow/src/state.rs`, `buyer.rs`, `vendor.rs`, `funding.rs` | Shares are not re-proved on restore: snapshots are read only from the party's own sealed storage. Is that acceptable, or should restore re-prove? |
| Node session records sealed with ChaCha20-Poly1305 under the vault key; secrets derived again from the seed (spend tier) | `crates/node/src/escrow_store.rs`, `escrow_buyer/state.rs`, `escrow_vendor/sale.rs` | A snapshot plus the receive tier can show *what* an escrow is but cannot sign. |
| Buyer round 1 is never stored; a restart opens again with fresh nonces and an attempt number, and the vendor answers each new attempt with fresh nonces | `escrow_buyer.rs` `reopen`, `escrow_vendor.rs` `vendor_open` | A buyer can make the vendor run round 2 again for the same terms and a different outpoint (with a higher attempt). The vendor then holds round 2 for several outpoints of the same terms but finishes funding only for the latest. Is round-2 output for an abandoned outpoint harmful? It contains the vendor's refund and buyer-wins partials for that outpoint. |
| Release nonce lost on restart: the vendor sends a fresh one, the buyer signs the same sighash again with fresh nonces | `escrow_vendor.rs` `vendor_release`, `escrow_buyer.rs` `buyer_release_nonce` | A release that does not verify against the held nonce consumes that nonce and gets a fresh one. Confirm that one buyer partial can never be combined with two vendor nonces. |
| Durable ruling store: one sealed file per (terms digest, outpoint), created by hard link | `crates/node/src/escrow_rulings.rs` | Atomicity on the target filesystems; behavior on filesystems without hard links (the store fails closed). |
| M3, option (a): the vendor claim pays exactly the vendor-wins outputs, moderator fee included | `crates/escrow/src/tx.rs` | Removes the moderator's fee incentive toward buyer-wins. Cost: an undisputed claim still pays the fee and shows the moderator's script. Option (b), a fee bond output in the funding, was rejected: it adds a moderator-spendable output to the funding. |
| M6: dispute letters are rate-limited before the chain lookup and the proofs (20 burst, one per 6 s; 4 distinct letters per escrow per 24 h); an already admitted or already verified letter is free | `crates/escrow/src/moderator.rs` `review_gated`, `crates/node/src/escrow_limits.rs` | The cheap checks before the gate (terms, claimant signature, template) need only keys the writer made up. Batch verification is not possible with `(c, z)` proofs; a commitment-form encoding would allow one multi-exponentiation per share (a format change). |
| Moderator funding check through `FundingChain::funding_output(outpoint, script, not_before)`, answered by following the funding script from `refund_height - max_refund_horizon` | `crates/escrow/src/letter.rs`, `crates/node/src/escrow_moderator.rs` `ChainView`, `chain_filters/` | Compact block filters cannot look a txid up. A letter waits while the filters scan. Only confirmed funding counts. |
| Presigned fee from the buyer's estimate times the largest spend's vsize, clamped to the policy floor and cap; CPFP through the 240-sat P2A anchor | `crates/node/src/escrow_fee.rs`, `escrow_bump.rs` | The child spends node-wallet coins only. It needs the passphrase again and a fee cap. |
| Coin locks on the funding PSBT's inputs from round 1 until broadcast | `crates/node/src/coin_locks.rs` | Stops a second payment from spending an input after both parties signed for the outpoint. |
| Replay guard checked before a step and recorded after it; vendor disputes have their own mail kind and sender role | `crates/node/src/escrow_mail.rs`, `letter_auth.rs`, `crates/tormail/src/guard.rs` | A letter is recorded as seen only after its step succeeded. A deferred or crash-interrupted letter runs again, so every step must be idempotent. |

## Known limitations (not asked to fix, but to weigh)

- Presigned spends are version 2, not TRUC. A CPFP child can be pinned by a large low-fee descendant.
- The moderator and its keys are a public testnet4 fixture (`escrow_fixture.rs`). The demo store mailbox secrets are public fixtures too.
- Escrow mail travels over loopback HTTP between nodes, not Tor. Letters are sealed (HPKE) and signed per conversation, and the drop is rate-limited.
- Per-order keys are hardened derivations from the wallet seed (`m/1984'/coin'/1'/index'`, `crates/node/src/seed.rs`), so anyone holding the seed holds every escrow key. A key index is reserved on disk before it is used, and is never reused.
- A buyer must be online to dispute before `D`. A shipping attestation is not a cryptographic event, so there is no second presigned set.

## Questions we most want answered

1. Soundness and zero-knowledge of the bitwise verifiable encryption as implemented, including challenge derivation and context binding (`vencrypt.rs`, `proofs.rs`).
2. Whether restoring from a snapshot without re-proving the shares is safe given the storage model.
3. Whether repeating round 2 for a new outpoint (higher attempt) leaks anything that matters.
4. Nonce handling across restarts, especially release re-signing.
5. Whether the adaptor-signature construction lets the favored party, the losing party, or the moderator finish a branch it should not, given MuSig2 adaptor semantics in `musig2` 0.4.
6. Fee and pinning: whether the anchor plus version 2 is enough near `V`, or TRUC is required.

## How to run

```sh
cargo test --locked -p openbazaar-escrow              # protocol, proofs, snapshots, review regressions
cargo test --locked -p ob-node multinode              # three nodes over loopback HTTP
cargo test --locked -p ob-node multinode_durable      # restarts, retries, disputes, fees, filters
```

Test names follow the review IDs, for example `h1_*`, `m1_*`, `m6_*` in `crates/escrow/src/tests/review.rs` and `persistence.rs`. `every_node_restarts_before_every_round_and_the_escrow_still_funds_and_releases` restarts all three nodes between every mail round.
