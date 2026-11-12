# OpenBazaar

Censorship-resistant person-to-person trade for ordinary goods. This tree is the private-market client and the home node.

Phase 0 ships a signed fixture catalog and one store screen. The phone app and the desktop shell share that screen. The phone is the buyer. A vendor runs the desktop app, or a home node that stays awake.

## Test

```bash
cargo test
```

Print or serve the fixture catalog:

```bash
cargo run -p ob-node
cargo run -p ob-node -- serve
```

`serve` keeps the vault key in the OS keystore; pass `--vault passphrase`, or `--vault file` on a headless machine. To pair the web shop, open it and type the one-time code the node prints in its terminal. `--proxy socks5h://127.0.0.1:9050` sends rate and chain requests through Tor, and `--no-rates` turns off prices. On testnet4, `--fixed-rates BTCUSD=N,ZECUSD=N` prices orders from the whole-dollar figures you give for 1 BTC and 1 ZEC and asks no rate server, so a node can price orders without the network, for tests and offline use; it is refused on mainnet. `--no-introductions` stops the node asking the directory's introductions over Tor, so it learns no stores or listings from the directory, does not announce its own store there, and pays no directory fee.

## Web app

```bash
cd apps/mobile
npm ci
npm run typecheck
npm test
npm run build
```

The export is written to `apps/mobile/dist`. The Tauri 2 shell in `apps/desktop` hosts that export.
