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

## Web app

```bash
cd apps/mobile
npm ci
npm run typecheck
npm test
npm run build
```

The export is written to `apps/mobile/dist`. The Tauri 2 shell in `apps/desktop` hosts that export.
