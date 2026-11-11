# Lightning direct pay

`ob-node serve --lightning` runs an embedded Lightning node ([LDK](https://lightningdevkit.org/) through `ldk-node`) so a store can take direct orders over Lightning, on test networks only (Mutinynet signet, testnet4, regtest). A mainnet node refuses Lightning until its connections run over Tor (see [Mainnet](#mainnet-is-blocked-until-tor)). Escrow stays on-chain (MuSig2, [escrow review packet](escrow-review-packet.md)); `"rail": "ln"` with escrow is refused at checkout.

The code is in `crates/lightning` (the `LightningRail` trait, the LDK implementation, a mock, and a regtest harness) and `crates/node/src/lightning` (routes, checkout, order status, wallet lock). Shared files only call into it.

## Design

| Choice | Decision |
|---|---|
| Implementation | `ldk-node` 0.7.1 in process, vendored in `vendor/ldk-node` with one change: its `rusqlite` range is widened to `>=0.31, <0.40` so it links next to Arti's. Remove the vendor copy when upstream widens the range. |
| Invoices | BOLT 11, one per order, 1 hour expiry, fresh payment hash. LSPS2 just-in-time channels work only with BOLT 11, so the store record's BOLT 12 offer stays encoding only. |
| Opt-in | Bit 1 of the rail-flags byte at the end of store record v3 (bit 0 is Monero, bit 2 Zcash; values above 7 are refused by the node and the client). `PUT /ln/stores` signs the record again; checkout reads only the signed flag. |
| Keys | `m/1984'/c'/6'/0'` from the wallet seed (`c` is 0 on mainnet and 1 on test networks), then HMAC-SHA512 with `"OpenBazaar Lightning seed v1"` for the LDK seed and `"OpenBazaar Lightning store v1"` for the state key. Restoring the seed restores the node key and on-chain Lightning funds; channels need LDK's state as well. |
| State | `wallets/node/lightning/sealed`: every LDK record is encrypted with ChaCha20-Poly1305 under the state key, with the record's namespace and key as associated data. The Lightning secret is in `wallets/node/ln-keys.obv`, sealed under the vault key. |
| Wallet lock | The secret is receive tier, so a locked node keeps issuing invoices and receiving. Paying needs a spend ticket from the passphrase (`POST /ln/spend-tickets`), bound to one order, hash, amount, and fee cap, cleared on lock. Opening and closing channels and sweeping take the passphrase in the request. LDK needs the key to stay running, so this is a policy gate on the node's routes, not a key the lock removes. |
| Mainnet | Refused until LDK's connections run over Tor. There is no mainnet LSP default. |

## Order flow

1. The seller turns Lightning on for a store (Settings → Lightning). The first time, the node answers `409` with an `lspNotice`; the client shows it and sends `acknowledgeLsp: true`. The choice is kept in `ln-settings.obv`.
2. `GET /catalog` sets `acceptsLightning` on a listing when the signed store record says so and the Lightning node is running. Checkout offers Lightning when every item in the seller's package has it.
3. Checkout with `"rail": "ln"` returns `payment.rail = "ln"` with the invoice, a `lightning:` URI for the QR code, the payment hash, the amount, `expiresAt`, and `jit` with the LSP fee cap when no channel can receive it yet.
4. The node follows LDK's payment events every 2 seconds. A settled payment marks the order Paid with `receivedMsat` and `lspFeeMsat`; an expired unpaid invoice shows `expired`.
5. The buyer pays from any Lightning wallet, or from its node: `POST /ln/orders/{id}/pay/quote`, then a spend ticket with the passphrase, then `POST /ln/orders/{id}/pay` (202). An order the same node issued (a demo store or the own store) is refused: pay it from another wallet.

Routes: `GET /ln/status`, `GET|PUT /ln/stores`, `POST /ln/address`, `POST /ln/channels`, `POST /ln/channels/{id}/close`, `POST /ln/sweep`, `POST /ln/spend-tickets`, `POST /ln/orders/{id}/pay/quote`, `POST /ln/orders/{id}/pay`. `GET /network` reports `lightning` (balances, channels) and `lightningBackend` (LSP, chain, gossip, what each reveals).

## Mainnet is blocked until Tor

`ldk-node` has no proxy support, so Lightning peers, the LSP, and the chain and gossip servers see the node's IP address. That breaks the rule that users are never locatable, so a `--network mainnet` node refuses Lightning everywhere with "Lightning on mainnet is disabled until its connections run over Tor; it would reveal this node's IP address.":

| Entry point | On mainnet |
|---|---|
| `--lightning` and every other Lightning flag (`--lsp`, `--no-lsp`, `--ln-chain`, …) | `ob-node serve` exits with the message |
| `enable_lightning`, `set_lightning` | refused, even with a test-network config |
| `GET /ln/status`, `GET /ln/stores`, `GET /network` `lightning` and `lightningBackend` | `{"enabled": false, "blocked": "…"}` |
| Every other `/ln/*` route (store opt-in, channels, sweep, quote, ticket, pay) | `403` with the message |
| The own store record | signed with the Lightning bit clear, whatever was set before |
| Checkout | `acceptsLightning` is `false` on every listing even if a store record sets the bit, `GET /catalog` `lightning.blocked` names why, and `"rail": "ln"` is refused; no invoice is issued |

The web shop hides the Lightning checkout option and the wallet card on mainnet and says why on the Lightning settings page and at checkout. It also drops Lightning if a node reports mainnet Lightning (`network: "bitcoin"`).

## The LSP and what it sees

A new store has no inbound channel. With an LSP (LSPS2), the invoice carries a route hint through the LSP; when the first payment arrives the LSP opens a channel to the store and keeps a fee out of the payment. The node refuses a JIT fee above 5 % of the payment or 10,000 sats, whichever is higher (`--lsp-max-fee-sats`). Later payments use the channel without a fee while it has room.

OpenBazaar runs no LSP. The default on test mode is the public Mutinynet one; testnet4 and regtest have none, and there is no mainnet default. With `--no-lsp` the seller opens channels by hand (`POST /ln/channels`) and needs inbound liquidity from somewhere else.

| Party | Sees | Does not see |
|---|---|---|
| LSP | Amount and time of every payment to the store, the channels it opens, the node's IP (clearnet) | What was bought, the buyer, the ship-to |
| Lightning peers | The node's IP address | Order contents |
| Routing hops | Forwarded amounts, timing, neighbors | Order contents |
| Buyer (from the invoice) | The store's Lightning node ID and the LSP | The store's IP address |
| Chain server (`--ln-chain`) | The Lightning wallet's addresses and channel outputs | Payments inside channels |
| Gossip server (RGS) | The node's IP address | Payments |

`ldk-node` has no SOCKS support, so `--proxy` does not cover Lightning peers, the chain server, or the gossip server. The Lightning node ID is separate from the store key. The Settings page shows this list and the one-time notice.

## Networks

| Mode | Lightning network | Chain view | LSP | Status |
|---|---|---|---|---|
| `--network testnet4` (default) | Mutinynet signet (`--ln-network signet`) | `mutinynet.com/api` | C= LSPS2 `0371d6fd…f5ef2b@44.219.111.31:39735`, token `JZWN9YLW` | Invoices with a JIT route issued from the live LSP. Its minimum JIT fee is 10,000 sats. |
| `--network testnet4 --ln-network testnet4` | testnet4 | `mempool.space/testnet4/api` | none: pass `--lsp` or `--no-lsp` | No public testnet4 LSP was found (November 2026). |
| `--network testnet4 --ln-network regtest` | regtest | `--ln-chain bitcoind:…` | `--lsp` or `--no-lsp` | Used by the integration tests. |
| `--network mainnet` | none | — | — | Refused until Lightning runs over Tor. |

Mutinynet coins and the on-chain Lightning wallet are separate from the node's testnet4 wallet, so `POST /ln/sweep` refuses to send them to it.

### Mainnet LSP choices (for when mainnet opens)

The operator will pick one; none is a default:

- Olympus by ZEUS: LSPS2 at `45.79.192.236:9735` (node ID `031b3013…e581`; take the full ID from ZEUS's documentation).
- Megalith, Breez, and LQwD: each runs LSPS2 service; get the node ID, address, and any token from the operator.

Check the LSP's current fees and channel sizes before choosing; they differ a lot.

## Setup

```sh
# Test mode, Mutinynet, public LSP:
ob-node serve --lightning

# Your own LSP, or none:
ob-node serve --lightning --lsp NODE_ID@HOST:PORT --lsp-token TOKEN
ob-node serve --lightning --no-lsp

# Testnet4 or regtest (no default LSP):
ob-node serve --lightning --ln-network testnet4 --no-lsp
ob-node serve --lightning --ln-network regtest --ln-chain bitcoind:USER:PASS@127.0.0.1:18443 --lsp NODE_ID@HOST:PORT
```

Other flags: `--ln-chain esplora:URL|electrum:URL|bitcoind:USER:PASS@HOST:PORT`, `--ln-rgs URL` or `--ln-p2p-gossip`, `--ln-listen HOST:PORT` (no inbound peer connections by default), `--lsp-max-fee-sats N`. Without `--lightning`, any of these is an error.

## Sizing just-in-time channels

The LSP keeps a channel reserve, anchor outputs, and the commitment fee out of the channel it opens, so a channel sized just above the payment cannot carry it. LSPs over-provision; the regtest LSP in `crates/lightning/src/regtest.rs` opens twice the payment. With 10 % over-provisioning a payment around 20,000 sats failed with `RouteNotFound`.

## Tests

- `cargo test -p openbazaar-lightning`: sealed store, key derivation, config parsing, the mock rail.
- `cargo test -p ob-node lightning`: the node against the mock (LSP notice and signing, a locked node issuing an invoice and turning the order Paid, paying with a bound ticket, passphrase on channel work, refusals, the secret in the receive tier, mainnet refusing every entry point while the test networks still run).
- Regtest with real LDK nodes (needs `bitcoind` on `PATH`, skipped otherwise): `crates/lightning/tests/regtest_jit.rs` and the node test `lightning::regtest_tests`. A buyer node opens a channel to an LSPS2 LSP; the seller node, locked, issues an invoice; the payment opens a JIT channel and the order turns Paid with the LSP fee recorded; quote and close answer `423` while locked and a wrong passphrase `400`.
- `crates/lightning/tests/signet_live.rs` (ignored): asks the live Mutinynet LSP for a JIT invoice.
- `apps/mobile`: `src/lightning.test.ts` and `src/records.test.ts` (rail-flags byte, no Lightning offered by a node that refuses it).

## Not done

- Remote sellers: invoices come from a seller wallet on this node, as for Monero. A remote seller's terms reply has to carry the invoice.
- Spliced or second channels from the LSP, LSPS1 channel purchase, and BOLT 12 offers.
- Tor for Lightning peers, the LSP, and the chain and gossip servers. Mainnet Lightning stays off until then.
