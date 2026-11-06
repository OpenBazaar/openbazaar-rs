# Buyer payment browser checks

From the repository root:

```sh
cargo build -p ob-node --locked
cd apps/mobile
npm ci
npx playwright install chromium
npm run typecheck
npm test
npm run build
npm run test:e2e
```

The test runner launches the real `ob-node serve 48187 --network testnet4
--data-dir <new temporary directory>` and a static/proxy web server on port
48173. It does not use `npm run dev`, port 8787, port 4173, or existing node data.
A test-only same-origin proxy supplies the existing desktop loopback-origin
injection and forwards pairing headers. The node process and temporary directory
are cleaned up when the runner stops.

Optional overrides: `OB_E2E_UI_PORT`, `OB_E2E_NODE_PORT`, and `OB_NODE_BIN`.
Both ports must be distinct spare ports. Build the web export before running.

The node runs with `--vault file` so it never touches the OS keychain, and the
server injects the node's `pairing.token` into the page the way the desktop app
does (a browser shop would otherwise need a code from the node's terminal).

The required smoke test uses the actual `/network`, `/lock`, `/setup`,
`/setup/confirm`, `/wallet`, `/wallet/address`, `/checkout`, `/orders`, and
`/orders/{id}/pay/quote` node routes. It sets the wallet passphrase, reads the
backup phrase once and types back the requested words, then asserts the testnet4
badge, receive address/QR, a new receive address, insufficient-balance
protection, checkout's Awaiting payment status, and the pairing header on the
real quote request. Because the disposable wallet has no coins, the test supplies
only a funded **UI wallet response** before pressing Pay. The real quote fails
against a local Esplora error stub, so no dialog opens and no `/pay` request is
sent; no transaction or chain confirmation is claimed.

Additional browser scenarios intercept the lock, quote, and ticket responses
(the node's own checks are in the Rust tests) to cover unlocking before the
step-up, the step-up dialog showing the node's quoted amount, fee cap, and
address, mainnet's `confirm` flag (no mainnet requests reach a real node),
partial-payment QR amounts, received amounts, transaction explorer links, status
polling, paid-button removal, network mismatch, and read-only escrow. A two-seller
checkout verifies one quote, one ticket, and one `/pay` call per order, never a
combined transaction.

For manual on-chain verification, run a separate testnet4 node with a fresh data
directory and its normal chain server, fund its wallet with test coins, purchase a
direct-pay item, and pay in the app. Verify `payTxid`, `Payment seen`, and then
`Paid` after the required confirmations. Also verify wallet sync errors, the
external-wallet address/amount QR, partial funding, and balance refresh. Phone UI
is bundle-checked; actual device wallet/clipboard behavior needs device testing.

The settings browser checks also cover separated shopping/settings navigation,
keyboard dismissal, persistent filter choices, blocked words with safe mode off,
hidden-listing recovery, the separate store-label page, and a 320 px mobile drawer.

The layout walkthrough visits browsing, search and empty states, product detail,
saved goods, bag, address creation/editing, physical checkout, receipt, order
payment, wallet, filters and store labels at 320, 768 and 1366 px. It checks
horizontal overflow, outer padding, square QR codes and active-button contrast.
It also verifies checkout includes the saved apartment/suite, Buy now returns to
the product, keyboard search works from settings, and local filters remain
accessible when the catalog is unavailable. Mainnet confirmation is also checked
at 320 px with requests intercepted. No real mainnet transaction is submitted.
