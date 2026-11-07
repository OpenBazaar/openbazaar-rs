# Home node protocol, version 2

The wire format between a home node and its paired devices (the native
OpenBazaar app, the desktop app in client mode, Tor Browser). The design and
threat model are in [home-node.md](home-node.md). Every encoding here has a
test vector shared by the Rust and TypeScript tests
(`crates/node/src/remote/vectors.rs`, `apps/mobile/src/homeNode/vectors.test.ts`).

Hex is lowercase. Base32 is RFC 4648 lowercase without padding. JSON field
names are camelCase. All times are unix seconds.

## Keys

| Key | Type | Held by | Use |
|---|---|---|---|
| Node key | X25519 | node vault (`remote-devices.obv`) | LAN channel, pairing transcript, `nodeId` |
| Admin onion | Tor v3 (ed25519) | node, Arti keystore | `http://{onion}/`, restricted discovery |
| Device key | P-256 ECDSA (`p256`) or BIP 340 (`bip340`) | device: Secure Enclave / StrongBox (`p256`), JS (`bip340`) | signs every request |
| Watch key | same types, optional | device, without user authentication | signs background `POST /remote/sync` only |
| Discovery key | X25519 | device's Tor client (or node-issued for Tor Browser) | Tor client authorization |
| Pairing secret | 80 random bits | shown on the node | single use, 10 minutes |

- `nodeId` = hex of the first 16 bytes of SHA-256(`"OBNODE2" ‖ nodePublic`).
- A device public key is written `p256:{33-byte SEC1 compressed, hex}` or
  `bip340:{32-byte x-only, hex}`. This string is the **key text**.
- `deviceId` = hex of the first 8 bytes of SHA-256(key text).
- A discovery public key is written as Tor writes it:
  `descriptor:x25519:{base32 of 32 bytes}` (Tor prints the base32 in upper
  case; either case is accepted). A discovery secret (for Tor Browser's
  onion-authentication prompt) is the base32 of the 32 secret bytes, clamped
  as an X25519 secret when used.

### Signatures

The device signs the UTF-8 bytes of a **preimage**:

- `bip340`: BIP 340 signature (64 bytes) over SHA-256(preimage).
- `p256`: ECDSA P-256 with SHA-256 over the preimage (what
  `SecKeyCreateSignature(.ecdsaSignatureMessageX962SHA256)` and Android
  `SHA256withECDSA` produce), DER encoded. Low-S is not required.

Signatures travel as hex.

## Pairing code

The secret (10 bytes) is shown three ways:

- **Words**: 8 words from the BIP 39 English list. 88 bits = the 80 secret
  bits followed by the first 8 bits of SHA-256(secret); each word is 11 bits,
  most significant first. Typed words are matched case-insensitively, by the
  first four letters.
- **QR / link**: `openbazaar://pair?v=2&s={secret base32}&k={nodePublic hex}&o={admin onion, 56 chars}&l={host:port}`.
  `l` is optional and may repeat (LAN addresses of the node's LAN listener).
- **Console**: both, plus the QR drawn in the terminal.

The **pairing onion** is derived from the secret, so 8 typed words are enough
to reach the node from anywhere: seed = HKDF-SHA256(ikm = secret, salt =
`"OBPAIR2"`, info = `"onion"`, 32 bytes) is an ed25519 secret key (RFC 8032
seed); its public key `A` gives the onion address
`base32(A ‖ SHA3-256(".onion checksum" ‖ A ‖ 0x03)[0..2] ‖ 0x03)`. The node
runs this onion service, unrestricted, only while the code is live; it serves
`/pair/*` and nothing else.

A code is live 10 minutes. At most 4 are live. The first valid request takes
the code (later ones get `409 code_taken`); approving or rejecting it ends the
code. After 20 bad proofs in 10 minutes pairing pauses (`429 paused`).

## Pairing

Reachable on the pairing onion, on the LAN channel, and on the node's
loopback gateway (tests).

**1. Request.** `POST /pair/request`

```json
{
  "v": 2,
  "key": "p256:02…",
  "watchKey": "p256:03…",
  "discoveryKey": "descriptor:x25519:…",
  "name": "Brian's iPhone",
  "platform": "ios",
  "proof": "…",
  "signature": "…"
}
```

`watchKey` and `discoveryKey` are optional. `platform` is `ios`, `android`,
`desktop`, or `browser`. `name` is 1 to 40 characters. The **transcript** is

```
OBPAIR2
{key}
{watchKey or ""}
{discoveryKey or ""}
{name}
{platform}
```

(lines joined by `\n`, no trailing newline). `proof` = HMAC-SHA256(key =
secret, transcript), hex: the secret never crosses the wire. `signature` is the
device key's signature over the transcript (proves it holds the key).

Answer `202`:

```json
{ "requestId": "{32 hex}", "nodeKey": "{nodePublic hex}", "sas": ["🦊","🌵","🚲","🍋"], "sasNames": ["fox","cactus","bicycle","lemon"], "expiresInSecs": 600 }
```

The device checks `nodeKey` against `k` when it came from a QR link, computes
the emoji itself, and stops if they differ from `sas`.

**2. Short authentication string.** Both screens show 4 emoji:
h = SHA-256(`"OBSAS2\n"` ‖ secret ‖ nodePublic ‖ UTF-8(transcript)); emoji
*i* = `SAS_EMOJI[h[i] & 63]` for i = 0..3. The list is fixed:

```
🐶 dog 🐱 cat 🦊 fox 🐻 bear 🐼 panda 🐨 koala 🐯 tiger 🦁 lion
🐮 cow 🐷 pig 🐸 frog 🐵 monkey 🐔 chicken 🐧 penguin 🐦 bird 🦉 owl
🦋 butterfly 🐢 turtle 🐙 octopus 🦀 crab 🐳 whale 🐬 dolphin 🐟 fish 🦈 shark
🌵 cactus 🌲 tree 🌻 sunflower 🍄 mushroom 🌙 moon ⭐ star ☀️ sun ⚡ lightning
🔥 fire 💧 droplet 🌈 rainbow ❄️ snowflake 🍎 apple 🍋 lemon 🍌 banana 🍇 grapes
🍓 strawberry 🥕 carrot 🌽 corn 🍕 pizza 🎂 cake ☕ coffee 🚲 bicycle 🚗 car
🚀 rocket ⛵ sailboat 🎈 balloon 🎁 gift 🔑 key 🔔 bell 🎸 guitar 📚 books
⚓ anchor 🧭 compass ⏰ clock 💡 bulb 🔒 lock ✂️ scissors 🧩 puzzle 👑 crown
```

**3. Approve on the node.** The owner compares the emoji and approves on the
node: the `ob-node pair` console prompt, the node's own app (loopback),
another paired device, or the LAN setup page while no device is paired
(`POST /setup/decide`, a same-origin form: `id={requestId}&approve=1|0`).

- `GET /remote/pending` → `{ "pending": [{ "requestId", "name", "platform", "sas", "sasNames", "deviceId", "ageSecs" }] }`
- `POST /remote/pending/{requestId}` `{ "approve": true }` → `{ "device": {…} }`

**4. Result.** `GET /pair/status/{requestId}` (long poll, up to 25 s) →
`{ "state": "pending" | "rejected" | "expired" }`, or

```json
{
  "state": "approved",
  "deviceId": "…",
  "nodeId": "…",
  "onion": "{admin onion}.onion",
  "discoverySecret": "…",
  "lan": ["192.168.1.20:8790"]
}
```

`discoverySecret` is present only when the request had no `discoveryKey` (the
node made one). Apps and client nodes send their own `discoveryKey`, so the
secret never leaves the device. The `requestId` is a 128-bit bearer secret
known only to the requester. After approval the device's discovery key is in
the admin onion's restricted-discovery list; Arti republishes the descriptor,
which can take a minute.

**Browser credential.** A paired device or the node can mint a ready-made
device for Tor Browser: `POST /remote/browser-credential { "name" }` →
`{ "deviceId", "accessKey": "obkey-…", "discoverySecret", "onion" }`. The user
types the discovery secret into Tor Browser's onion-authentication prompt and
the access key into the shop. The access key is the BIP 340 device secret
(base32, 52 letters, after `obkey-`).

## Signed requests

Every admin request carries:

| header | value |
|---|---|
| `X-Ob-Device` | `deviceId` (of the device key or its watch key) |
| `X-Ob-Time` | unix seconds |
| `X-Ob-Nonce` | 32 random hex digits |
| `X-Ob-Signature` | signature over the preimage, hex |

Preimage: `OBRQ2\n{nodeId}\n{deviceId}\n{time}\n{nonce}\n{METHOD}\n{path}\n{SHA-256(body) hex}`.
`path` is the request target exactly as sent, query string included.

The node takes a time within 120 s of its clock and not before it started, and
each nonce once per device (kept 240 s, 4096 per device). Refusals are JSON
`{ "error", "code" }`:

| status | code | meaning |
|---|---|---|
| 401 | `unsigned` | missing or bad signature |
| 401 | `unknown_device` | revoked or never paired: forget the node |
| 401 | `replay` | nonce seen |
| 401 | `clock` | time out of window; `serverTime` included, retry once |
| 403 | `node_only` | not allowed to devices (see home-node.md) |
| 403 | `watch_only` | a watch key may only call `POST /remote/sync` |

While the node is moving to a new address, every answer to a signed request
carries `X-Ob-Next-Onion: {new onion}`; the device switches to it.

## Transports

**Tor.** HTTP/1.1 to `http://{admin onion}/` on port 80, one request per
stream, `Connection: close`. The client must hold its discovery key in its Tor
client's keystore, or it cannot find the service.

**LAN channel.** If the node runs with `--lan`, it listens on port 8790 (all
interfaces) and announces `_openbazaar._tcp.local.` over mDNS with the host
name `openbazaar.local.` and a random instance name; no TXT data identifies the
node. Nothing on this port is plaintext except the setup page. A request is
`POST /lan` with `Content-Type: application/octet-stream` and the body
`E ‖ C`:

- `E` = a fresh X25519 public key (32 bytes) per request.
- `K` = HKDF-SHA256(ikm = X25519(e, nodePublic), salt = `"OBLAN1"`, info = `E ‖ nodePublic`, 64 bytes); `Kreq` = K[0..32], `Kres` = K[32..64].
- `C` = ChaCha20-Poly1305(`Kreq`, nonce = 12 zero bytes, aad = `"OBLAN1"`, plaintext = the complete inner HTTP/1.1 request, signed as above).

The answer is `200` with body ChaCha20-Poly1305(`Kres`, zero nonce, aad =
`"OBLAN1"`, the complete inner HTTP/1.1 response). Any failure to decrypt is a
bare `400`. Only the holder of the node key can read the request or produce
the answer, so a device that pinned `nodePublic` at pairing knows it reached
its node. Replays fail on the inner nonce. `/pair/*` works on the LAN channel
too (the inner request is unsigned and the proof does the work).

Clients find the node by mDNS, by `openbazaar.local:8790`, or by the `lan`
addresses the node reports, and fall back to Tor after 1.5 s.

**Setup page.** `GET /` on the LAN port, only while no device is paired (or
for 10 minutes after `ob-node pair --lan-page`): the QR, the words, and, on
first run only, pending requests with their emoji and Approve/Reject. It has
no scripts (`Content-Security-Policy: default-src 'none'`).

## Sync

`POST /remote/sync` returns, in one round trip, what changed since the
device's replica:

```json
{ "have": { "catalog": { "doc": "{etag}", "items": { "{hash}": "{etag}" } }, "orders": { … }, "wallet": { "doc": "{etag}" } } }
```

Answer:

```json
{
  "serverTime": 1794063000,
  "node": { "nodeId", "onion", "nextOnion": null, "lan": ["…"], "version": { … } },
  "collections": {
    "catalog": { "doc": { "etag", "value" } | null, "upsert": [ { "id", "etag", "value" } ], "remove": [ "{id}" ] },
    "wallet": { "error": { "status": 423, "code": "locked" } }
  }
}
```

| collection | source | items at | item id |
|---|---|---|---|
| `catalog` | `GET /catalog` | `listings` | `hash` |
| `orders` | `GET /orders` | `orders` | `id` |
| `stores` | `GET /stores` | `stores` | `key` |
| `wallet` | `GET /wallet` | (document) | |
| `lock` | `GET /lock` | (document) | |
| `network` | `GET /network` | (document) | |

The etag of an item or document is the first 16 hex digits of SHA-256 of its
JSON as the node serializes it. `doc` is the document without
its item array. A collection missing from `have` comes back whole. `doc` is
`null` when unchanged. A device applies `remove`, then `upsert`, then `doc`,
and stores the new etags; it never computes etags itself. `nextOnion`, when
set, is the address the node is moving to (see Revocation); the device should
switch to it.

## Batches

`POST /remote/batch` runs queued mutations in one round trip:

```json
{ "id": "{32 hex}", "requests": [ { "method": "PUT", "path": "/content-labels", "body": { … } } ] }
```

At most 16 requests, run in order through the same policy as single requests;
the batch stops at the first answer of 500 or above. Answer:
`{ "id", "results": [ { "status", "body" } ] }`. The node remembers each batch
`id` per device for 24 hours and answers a repeat with the stored results, so
a device can resend a batch whose answer it lost.

## Devices and push

- `GET /remote/devices` → `{ "devices": [ { "id", "name", "platform", "kind": "app" | "browser", "pairedAt", "lastSeen", "lastVia": "tor" | "lan" | "local" | null, "pairedBy", "push": { "enabled", "platform" }, "thisDevice" } ], "thisDevice" }`.
- `DELETE /remote/devices/{id}` (body optional: `{ "rotate": true }`): removes
  the device and every device it paired, drops their discovery keys from the
  admin onion, and with `rotate` starts an address move.
- `PUT /remote/push` `{ "platform": "apns" | "fcm", "token": "…" }` turns push
  on for the calling device; `DELETE /remote/push` turns it off. Off by default.

The node sends a wake-up only if it runs with `--push-gateway URL`:
`POST {URL}/v1/wake` `{ "platform", "token" }`, over Tor, at most once per
device per 10 minutes, 0 to 90 s after the event (new order, new message, a
payment seen). The wake-up has no content; the app syncs when it receives it.

## Revocation and address moves

Removing a device takes its keys off the device list at once (its next
request is `401 unknown_device`) and its discovery key off the admin onion.
Tor does not promise that a removed client loses the introduction points it
already knows, so `rotate` also moves the node to a fresh onion address: the
node runs both, sends `nextOnion` to every remaining device in sync, and
retires the old address once every remaining device has synced from the new
one, or after 7 days.

`ob-node devices` lists devices on the node console (it talks to the running
node on loopback), `ob-node devices --remove ID`
removes one, and `ob-node devices --remove-all --rotate` is the recovery path
when every device is lost.
