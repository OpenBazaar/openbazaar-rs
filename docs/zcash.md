# Zcash direct pay

Zcash as a direct-pay rail next to Monero and Lightning: a store opts in, each order gets its own shielded address, and the seller's node marks the order paid when the payment is deep enough. Escrow stays bitcoin-only (see [Escrow](#escrow-is-refused)).

Status: phase 1a, on the `zcash-direct` branch. The library and the store-record flag exist; no node route takes a Zcash order yet.

## Decisions

| Choice | Decision |
|---|---|
| Library | Upstream librustzcash, embedded: `zcash_client_backend` 0.25 (compact-block scanning), `zcash_keys` 0.17, `orchard` 0.16, `zcash_protocol` 0.11, `zip321` 0.10. One key store, as with monero-oxide; no wallet daemon (Zallet) beside the node. |
| Not used | `zcash_client_sqlite`: it links `libsqlite3-sys` 0.35 and Arti 0.48 links 0.37, and only one may link (the problem behind `vendor/ldk-node`); its database is also plaintext, and everything this node keeps at rest is sealed. The backend's `tor` feature: it brings arti-client 0.43, a second Tor client beside the node's. |
| Zakura's crates | `zakura-core/common` and `wallet-libraries` keep the upstream crate names, so switching is a `Cargo.toml` change. Not before an independent audit of the forked proving stack and a benchmark of sending (phase 3); receiving does no proving. |
| Pools | Ironwood is the default privacy pool and the only one that pays an order. Each order address carries one Orchard-protocol receiver, ZIP 316's only receiver type for that protocol; since NU6.3 (mainnet height 3,428,143) a payment to it lands in Ironwood. Orchard is outbound only: nothing new enters it, and its notes can only be spent, into Ironwood. The wallet still reads Orchard actions, with the same viewing key, so notes from before NU6.3 on a restored phrase show as their own balance, and so do their spends. An Orchard note at or after NU6.3 is not believed, since only a server that ignores consensus could send one. No Sapling receiver, and no transparent receiver, which would let a buyer pay in the clear. A buyer paying straight from an exchange that only sends transparent cannot pay. |
| Keys | ZIP 32 from the 64-byte BIP 39 seed of the wallet phrase (empty passphrase), account 0, coin type 133 on mainnet and 1 on testnet. That is how Zcash wallets derive their first account, so the phrase restores the funds in Zodl or any ZIP 32 wallet. ZIP 32's Orchard derivation (BLAKE2b, `ZcashIP32Orchard`) shares nothing with BIP 32's (`Bitcoin seed`), so these keys are independent of every role key in `seed.rs`. |
| Tiers | Receive: the UFVK, in its standard encoding, sealed in `zec-watch.obv`. It finds payments while the wallet is locked and, unlike Monero's view key, also computes nullifiers, so a spend made from the phrase restored elsewhere shows. Spend: the phrase in `spend.obv`; the spending key is derived for one signature (phase 3). |
| Per-order address | Diversified address `j` of the external scope, `j` from 1, never reused, saved before it is shown. Index 0 is the address a restored wallet shows first. Diversified addresses are unlinkable without the viewing key. A received note is matched to its order by its diversifier index, so a payment needs no memo. |
| Payment request | ZIP 321 `zcash:` URI carrying the amount only: no label, message, or memo, since whatever is in the URI lands in the payer's history. |
| Confirmations | 30 blocks, about 12.5 minutes at NU7's 25-second spacing. ZIP 315's 10 confirmations for outputs from others were set at 75 seconds per block; both networks are past NU7. |
| Amounts | Zatoshi (10^-8 ZEC), integer math. Owed = sats × 10^8 / sats-per-ZEC, rounded up so the seller is never paid less than the price. Kraken has ZEC/USD but no ZEC/BTC, so the rate is crossed with the node's BTC/USD (phase 1b). |
| Chain | Compact blocks over the lightwalletd protocol, from a Zaino or lightwalletd the user names (`--zec-indexer`); no default server. Trial decryption runs on the node, so the server cannot tell which outputs are ours; it sees the IP address and sync timing. Fetching a full transaction by id would tell it which one matters, so the rail never does. Own node: Zebra (or Zakura) plus Zaino on loopback. |
| Store opt-in | Bit 2 of the rail-flags byte at the end of store record v3 (bit 0 Monero, bit 1 Lightning). Decoders before this branch refuse a byte above 3 and then drop the whole record, so phase 1a ships the reader and no node sets the bit until releases that read it are out. |

### Escrow is refused

The bitcoin escrow presigns a refund before the buyer funds anything. Zcash's shielded pools have no scripts or timelocks, and a shielded spend proves that its note is in a note commitment tree root that already contains it, so no refund can exist until the funding confirms. FROST (ZIP 312, still a draft) would give a 2-of-3 with the moderator holding a share, which is the trust model this design removed; transparent P2SH multisig shows the moderator on chain. `"rail": "zec"` with escrow is refused, as for Monero and Lightning.

## Phases

### Phase 1: receive, for stores on this node

Buyers pay from any Zcash wallet with the `zcash:` URI. Exit: a demo store on testnet takes a Zcash order paid from Zodl, the order turns Paid at 30 confirmations, and a locked node does all of it.

| Step | What | Status |
|---|---|---|
| 1a | `crates/zec`: keys from a seed, per-order addresses, ZIP 321 URI, amounts, a view-key wallet that scans compact blocks with reorg handling and spend detection, Ironwood as the pool that pays orders and Orchard as outbound only, sealed encoding, a mock chain that mines real encrypted notes and follows NU6.3's rule. Store record bit 2 readable in the node and the web shop. | done |
| 1b | Node `zec` module beside `xmr`: the seed's BIP 39 bytes, `zec-watch.obv` and `zec-state.obv` per wallet, `PUT /zec/stores` (signs the record with bit 2), `acceptsZec` in `GET /catalog`, `"rail": "zec"` at checkout, order status (Awaiting payment, Payment seen, Paid at 30), ZEC rate, `GET /network` `zecBackend`. Tests against the mock. | next |
| 1c | Live chain: a `CompactTxStreamer` client (`GetLatestBlock`, `GetBlock`, `GetBlockRange`, `GetLightdInfo` to check the chain), over `--proxy` for anything not on loopback or the LAN, one SOCKS username per connection, no redirects. Mainnet refuses a remote server without `--proxy`. An ignored live testnet test. | |
| 1d | Web shop: the Zcash option at checkout, the QR from the URI, status and amounts, Zcash in store settings. | |

### Phase 2: sales to other nodes

The seller's terms reply carries the order address, the zatoshi amount, and the rate it used; the buyer accepts it within the same tolerance as a fiat price. Zcash is the easiest rail to do this for: issuing an address needs only the receive tier and no network call. The new-seller rule still applies: a store with no finished escrow sale declines direct pay from another node.

### Phase 3: pay from the node

Spend tickets bound to order, address, amount, and fee cap, as for Monero. Fees follow ZIP 317 and do not float, so the cap is simple. Outputs and change go to Ironwood. Orchard notes from before NU6.3 move to Ironwood through librustzcash's ZIP 318 migration (`zcash_pool_migration`: canonical denominations, spaced out), not as one-off sends, since a distinctive crossing amount would mark the wallet. Needs note witnesses (a shard tree kept in the sealed state) and Halo 2 proofs (seconds of CPU, no parameter download for Orchard and Ironwood). PCZT lets the node build and prove while the spending key stays in the spend tier or on the phone. Also: a mempool stream from the indexer, so an order shows Payment seen before its first block.

## Tests

- `cargo test -p openbazaar-zec`: key derivation against `zcash_keys`' own ZIP 32 path, address reuse across restarts, Ironwood payments on the mock chain, Orchard notes from before NU6.3 counted apart and spent out, a server that puts new value in Orchard, confirmations, reorgs, spends, encoding, URIs, amounts.
- `cargo test -p openbazaar-protocol`: the rail-flags byte with bit 2.
