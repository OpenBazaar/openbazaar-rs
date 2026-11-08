# Privacy hardening, 2026-11-08

Base: `origin/main` at `cb218885d123191dfb1f9bb26c17675348de7e60` ("Let a seller sign a coupon the store record already keeps"). The 2026-11-08 re-audit reviewed `0c0f2b5`, which is behind this commit. Uncommitted fixes in the other checkout were not assumed to be merged and were not copied over wholesale. This branch is `codex/privacy-hardening`.

## What main already did

These protections are on `cb21888` and were exercised again by the library suite below. They are not new in this branch.

- Wallet, address, and script queries stay off public Esplora, including the filter fallback.
- Coinbase, Kraken, and public Esplora are not dialed in the clear. `GET /network` warns and names `--proxy socks5h://127.0.0.1:9050` when a private route is missing.
- Public filter peers are not started unless `--proxy` or an explicit `--peer` is set.
- A public Esplora broadcast is not sent in the clear.
- An onion sale is one request; a redirect is not followed.

## Fixes in this branch

### 1. LAN response encryption

Replaying a sealed `POST /lan` used to encrypt the rejection, and every other answer, with the same response key and a 12-byte zero nonce. The request layout is unchanged: each request has a fresh ephemeral, so `Kreq` encrypts one plaintext.

Every answer, including replay, clock, and revoked-device errors, is now `0x02 ‖ 24-byte nonce ‖ XChaCha20-Poly1305(Kres, nonce, aad = "OBLAN2")`. The nonce mixes 16 OS-random bytes with an 8-byte counter stored at `{data_dir}/lan-response-counter` and `sync_all`'d. The counter starts at 1 and takes `max(memory, disk) + 1`, so a restart cannot return to a used nonce. Version other than 2, a short body, an all-zero nonce, and an AEAD failure are rejected. The old raw ciphertext is not decrypted.

This is a breaking change for LAN clients. Rust and TypeScript reject the old layout, `docs/home-node-protocol.md` describes the envelope, and `vectors.json` pins a non-zero response nonce. Production does not use that pinned nonce.

Regression: `a_captured_lan_envelope_cannot_reveal_an_earlier_answer` posts through `lan::respond`. XOR of the success ciphertext with the replay and clock answers does not recover the device id. The same test covers revoke, a process restart against the counter file, and malformed envelopes.

### 2. Image metadata

`prepare_image` runs before a new upload is hashed or stored. JPEG, PNG, GIF, and WebP are decoded with a dimension cap of 8192, a pixel cap of 16,777,216, a decoded-memory cap of 64 MiB, and a GIF frame cap of 300. EXIF orientation is applied, then identifying metadata (JPEG COM/APP1/APP13, PNG text/EXIF/time, GIF comment and application extensions, WebP EXIF/XMP) is dropped by re-encoding. An image that is already clean is stored unchanged, so its hash stays stable.

`GET /media/{hash}` still checks the stored bytes against the hash, then serves a sanitized copy when metadata remains. The file on disk is not rewritten, because rewriting would break the hash check. A legacy file that cannot be sanitized is not served. A replica that requires the original hash will not cache that cleaned response under the signed hash. New uploads hash the cleaned bytes.

Covered by the image-sanitize unit tests (formats, a GPS JPEG with orientation 6, the dimension limit, and a previously stored file) and `an_upload_drops_a_gps_note_before_it_is_hashed` through `POST /media`.

### 3. Outbound networking

The Esplora `reqwest` client uses `redirect(Policy::none())` and `no_proxy()`, then an explicit proxy only when one is configured. `canonical_server` parses the URL and dials that URL, so the host that is classified is the host that is contacted. `is_private_url` uses the same parser: unparseable input, a non-http(s) scheme, an empty host, or a backslash is not private. `http://127.0.0.1@mempool.space` is public. `http://mempool.space@127.0.0.1` is loopback.

`an_esplora_redirect_is_not_followed` binds two `127.0.0.1:0` listeners. The configured server returns 302 to the second. The second records zero requests. `private_servers_are_recognized` covers the userinfo and backslash cases.

The privacy warning still says public chain servers and rate hosts are not contacted without `--proxy`, and now also says port 9150 is Tor Browser's port and that `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY` are ignored. Setting those variables in this process and watching a dial was not done: cargo tests run in parallel, and a process-global proxy would race. The client builder calls `.no_proxy()` before any custom proxy. That is source-confirmed, not a live environment-variable test.

Lightning's own dial path was not given a new redirect test in this pass. Existing lightning tests in the library suite still pass. Do not read that as a measured Tor circuit.

### 4. Coin provenance

`blocked_coins` walks wallet history. External keychain receives are fresh. One foreign output script, with inputs that are fresh or already linked to that script, is linked to that script. Several foreign scripts, a link to someone else, a cycle, or depth past 64 is mixed. A transaction with no foreign output, such as an escrow fee-bump that drains to change, keeps the input provenance. An escrow anchor input that is not ours does not by itself create a link.

Initial payments, quotes, direct RBF, and escrow CPFP all refuse incompatible coins unless the request sets `linkCoins: true`. RBF still allows the payment's own inputs. The quote and the builders append: "Spending change from a payment to someone else would link the two on chain. Separate transactions, or separate outputs from the same funding, do not make those purchases unlinkable."

Provenance is the recipient script, not the seller name. A second checkout to the same seller uses a new address, so that change is not treated as the same recipient. The same-recipient case is tested with `build_payment` to the original address. The RBF test confirms the other seller's payment in the mock wallet before the bump, because BDK only adds confirmed extra inputs. The CPFP test builds a child with the other seller's change blocked.

Shared funding still links those coins on chain. This change does not claim otherwise.

### 5. External explorers

Web payments, sales, and the native payments screen no longer build a mempool.space link. Transaction ids are copyable. The note next to them says a browser does not inherit this node's Tor or proxy settings. The desktop app uses the web marketplace components, so it gets the same change. There is no "view on explorer" button.

### 6. Visible privacy status

The client keeps `proxy`, `clearnet`, `privacyWarning`, and `chainBackend` (including error and `fallbackActive`). The status line distinguishes a chain transport failure, an active public fallback, a missing private route, a proxy that has reached peers, and a proxy that is only configured. The wallet note stays outside the loaded-wallet branch, so a failed wallet load does not hide it. The network badge keeps "Testnet4 · test coins, no value" in its own element; the privacy status is a sibling.

## Validation

All of this used temporary directories, mocked chain sources, synthetic images, and spare ports. Nothing spent or broadcast real coins. `npm run dev` was not run. Ports 8787 and 4173 were not bound. The e2e UI listens on 48173 and the disposable nodes on 48187 and 48188.

| Command | Result |
|---|---|
| `cargo test -p ob-node --lib -- --skip live_testnet4` | 300 passed, 0 failed, 1 ignored (`live_testnet4_send`), 3 filtered by the skip. 54.15s. Repeated after the formatting-only files were restored. |
| `cargo build -p ob-node --bin ob-node` | Finished. Remaining warnings are pre-existing unused items in `escrow_outbox` and `letter_auth`. |
| `npx tsc --noEmit` in `apps/mobile` | Passed. |
| `npm test` in `apps/mobile` | 146 passed, 0 failed. |
| `npx playwright test e2e/payments.spec.ts` | 8 passed. The web bundle was produced with `npm run build` (`expo export --platform web`) because the e2e server serves `apps/mobile/dist`. The first run failed on a blank page before that export existed. |

The ignored live-send test was not run.

## Not verified

- A live Tor circuit, a real SOCKS proxy, or a process with `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` set.
- Lightning vendor or LSP clients following or refusing redirects, beyond the existing library tests.
- A dependency advisory scan.
- Native-app rendering. `NativePayments.tsx` was reviewed and typechecked. Playwright covered the web screens, which the desktop shell embeds.
- That a cleaned legacy image still replicates under its old signed hash. It will not. New uploads hash the cleaned bytes.
