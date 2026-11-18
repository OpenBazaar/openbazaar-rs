# Lightning direct pay

`ob-node serve --lightning` runs an embedded Lightning node ([LDK](https://lightningdevkit.org/) through `ldk-node`) so a store can take direct orders over Lightning. Public peers, Esplora, and gossip go through `--proxy` or the built-in `--tor` client, so they do not see this machine. Mainnet Lightning requires that path and an LSP you name (see [Mainnet](#mainnet)). Escrow stays on-chain (MuSig2, [escrow review packet](escrow-review-packet.md)); `"rail": "ln"` with escrow is refused at checkout. The one Lightning payment around escrow is a dispute filing fee ([below](#dispute-filing-fees)).

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
| Mainnet | Requires `--proxy` or `--tor`, and an LSP you name with `--lsp` (or `--no-lsp`). There is no mainnet LSP default. |

## Order flow

1. The seller turns Lightning on for a store (Settings → Lightning). The first time, the node answers `409` with an `lspNotice`; the client shows it and sends `acknowledgeLsp: true`. The choice is kept in `ln-settings.obv`.
2. `GET /catalog` sets `acceptsLightning` on a listing when the signed store record says so and the Lightning node is running. Checkout offers Lightning when every item in the seller's package has it.
3. Checkout with `"rail": "ln"` returns `payment.rail = "ln"` with the invoice, a `lightning:` URI for the QR code, the payment hash, the amount, `expiresAt`, and `jit` with the LSP fee cap when no channel can receive it yet.
4. The node follows LDK's payment events every 2 seconds. A settled payment marks the order Paid with `receivedMsat` and `lspFeeMsat`; an expired unpaid invoice shows `expired`.
5. The buyer pays from any Lightning wallet, or from its node: `POST /ln/orders/{id}/pay/quote`, then a spend ticket with the passphrase, then `POST /ln/orders/{id}/pay` (202). An order the same node issued (a demo store or the own store) is refused: pay it from another wallet.

Routes: `GET /ln/status`, `GET|PUT /ln/stores`, `POST /ln/address`, `POST /ln/channels`, `POST /ln/channels/{id}/close`, `POST /ln/sweep`, `POST /ln/send/quote`, `POST /ln/send`, `POST /ln/withdraw/quote`, `POST /ln/withdraw`, `POST /ln/spend-tickets`, `POST /ln/orders/{id}/pay/quote`, `POST /ln/orders/{id}/pay`. `GET /network` reports `lightning` (balances, channels) and `lightningBackend` (LSP, chain, gossip, what each reveals).

## Mainnet

Public Lightning peers, an Esplora chain source, and Rapid Gossip Sync are dialed through `--proxy` or the SOCKS port built-in `--tor` opens. The node refuses a public host that would be dialed in the clear. Electrum, bitcoind, and `--ln-listen` stay direct, so a public one of those is refused too. With `--remote`, inbound Lightning is an onion on the node's Tor client, forwarded to a loopback socket that is not announced.

A mainnet node with no proxy and no built-in Tor refuses Lightning with "Lightning on mainnet is disabled until its connections run over Tor; it would reveal this node's IP address." With `--proxy` or `--tor`, plus `--lsp` or `--no-lsp`, mainnet Lightning runs. There is no default mainnet LSP.

| Entry point | Mainnet without a Tor path | Mainnet with `--proxy` or `--tor` |
|---|---|---|
| `--lightning` without `--lsp` or `--no-lsp` | exits: there is no default LSP | exits: name an LSP or pass `--no-lsp` |
| `--lightning` and a public peer, Esplora, or gossip server | exits: that host would see this machine | dials that host through Tor |
| `GET /ln/status` and checkout | `{"enabled": false, "blocked": "…"}`, no Lightning offered | Lightning is offered; `network` is `bitcoin` |
| A public `--ln-listen`, Electrum, or bitcoind | refused | refused |

The shop shows the node's `blocked` string and hides Lightning while it is set. It offers Lightning on mainnet when the node reports `network: "bitcoin"` and no `blocked` string.

## The LSP and what it sees

A new store has no inbound channel. With an LSP (LSPS2), the invoice carries a route hint through the LSP; when the first payment arrives the LSP opens a channel to the store and keeps a fee out of the payment. The node refuses a JIT fee above 5 % of the payment or 10,000 sats, whichever is higher (`--lsp-max-fee-sats`). Later payments use the channel without a fee while it has room.

OpenBazaar runs no LSP. The default on test mode is Megalith's public Mutinynet one (the one Alby Hub uses); testnet4 and regtest have none, and there is no mainnet default. With `--no-lsp` the seller opens channels by hand (`POST /ln/channels`) and needs inbound liquidity from somewhere else.

| Party | Sees | Does not see |
|---|---|---|
| LSP | Amount and time of every payment to the store, the channels it opens, a Tor exit when `--proxy` or `--tor` is set | What was bought, the buyer, the ship-to |
| Lightning peers | A Tor exit when `--proxy` or `--tor` is set | Order contents |
| Routing hops | Forwarded amounts, timing, neighbors | Order contents |
| Buyer (from the invoice) | The store's Lightning node ID and the LSP | The store's IP address |
| Chain server (`--ln-chain`) | The Lightning wallet's addresses and channel outputs | Payments inside channels |
| Gossip server (RGS) | A Tor exit when `--proxy` or `--tor` is set | Payments |

The desktop app starts built-in `--tor` and does not pass a separate `--proxy`. That Tor client is the proxy Lightning, rates, and the other public clients use. Without either, public peers and servers are not contacted. The Lightning node ID is separate from the store key. The Settings page shows this list and the one-time notice.

## Dispute filing fees

On a v5 escrow, whoever opens a dispute pays the moderator a filing fee outside the escrow. The moderator's fee request carries a BOLT 11 invoice only when one of its own usable channels has inbound room for the whole fee; it never asks the LSP for a just-in-time channel, because that would cost an LSP round trip for every reviewed letter and the LSP's fee could be larger than the filing fee. Otherwise the request carries only an on-chain script. An invoice lives one hour; after that the filer pays on-chain until the request itself lapses and the moderator sends a new one.

The filer pays the invoice from its own Lightning node like any remote invoice (`ln_can_pay_fee`, `ln_pay_fee`), with the routing fee cap shown in the quote. The moderator credits only what its own node received for that payment hash, at least the fee in full; the filer's preimage is its own proof and is never what the moderator trusts. The moderator's profile warns when filers would pay on-chain: Lightning is off, or no channel has room for the fee (a fee under the LSP fee cap could not pay for a new channel anyway).

| Party | Sees | Does not see |
|---|---|---|
| Moderator | The fee's payment hash and amount, as for any invoice it issues | The filer's Lightning node, unless the route reveals it |
| Routing hops | The fee amount and timing | That it is a dispute fee |
| LSP | Nothing new: no JIT channel is opened for a fee | |

## Networks

| Mode | Lightning network | Chain view | LSP | Status |
|---|---|---|---|---|
| `--network testnet4` (default) | Mutinynet signet (`--ln-network signet`) | `mutinynet.com/api` | Megalith LSPS2 `03e30fda…9d7579@64.23.192.68:9736`, no token | A JIT invoice was issued over Tor from the live LSP (November 2026). The C= LSP (`0371d6fd…f5ef2b`) no longer answered at either published address. |
| `--network testnet4 --ln-network testnet4` | testnet4 | `mempool.space/testnet4/api` | none: pass `--lsp` or `--no-lsp` | No public testnet4 LSP was found (November 2026). |
| `--network testnet4 --ln-network regtest` | regtest | `--ln-chain bitcoind:…` | `--lsp` or `--no-lsp` | Used by the integration tests. |
| `--network mainnet --tor --lsp …` or `--no-lsp` | bitcoin | an Esplora or your own node you name with `--ln-chain` | the LSP you name, or none | Runs. Public chain and gossip go through Tor. |

Mutinynet coins and the on-chain Lightning wallet are separate from the node's testnet4 wallet, so `POST /ln/sweep` refuses to send them to it. `POST /ln/withdraw/quote {address, amountSats}` (or `{address, max: true}`) then `POST /ln/withdraw {quote, passphrase, confirm: true}` send them to a Mutinynet address the user names instead; close a channel first to bring its funds on chain. LDK picks the on-chain fee when it sends, so the quote cannot show it. `POST /ln/send/quote {invoice}` then `POST /ln/send` pay a pasted invoice with the order fee cap. Both use the bitcoin send's quote, passphrase and confirm steps ([`wallet-lock.md`](wallet-lock.md)) and are refused to paired devices.

### Mainnet LSP choices

The operator will pick one; none is a default:

- Olympus by ZEUS: LSPS2 at `45.79.192.236:9735` (node ID `031b3013…e581`; take the full ID from ZEUS's documentation).
- Megalith, Breez, and LQwD: each runs LSPS2 service; get the node ID, address, and any token from the operator.

Check the LSP's current fees and channel sizes before choosing; they differ a lot.

## Setup

```sh
# Test mode, Mutinynet, public LSP (what the desktop app runs):
ob-node serve --proxy socks5h://127.0.0.1:9050 --lightning

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
- `cargo test -p ob-node multinode_fee_tests::a_fee_the_moderators_channels_can_take_is_paid_over_lightning`: a filing fee paid over the mock rail and credited from the moderator's own node. No regtest test of the fee path yet.
- `apps/mobile`: `src/lightning.test.ts` and `src/records.test.ts` (rail-flags byte, no Lightning offered by a node that refuses it).

## Not done

- Spliced or second channels from the LSP, LSPS1 channel purchase, and BOLT 12 offers.
- Electrum, bitcoind, and a public `--ln-listen` stay off Tor. Point those at this machine or the LAN, or use Esplora through the proxy.
