# The phone app for a home node

The Expo app in `apps/mobile` is also the native client of protocol v2
(`docs/home-node-protocol.md`): it pairs with a home node, keeps an encrypted
copy of the store, and talks to the node over Tor, or directly on home Wi-Fi.
The web shop (`App.web.tsx`) is unchanged; on a phone the shop opens from
Settings > Marketplace.

## Where things are

| Part | Code |
| --- | --- |
| Embedded Tor (Arti) | `crates/mobile-tor` (C ABI for iOS, JNI for Android), built by `scripts/build-mobile-tor.sh`; Expo module `apps/mobile/modules/ob-tor` |
| Device keys | `modules/ob-device-key`: Secure Enclave / Android Keystore (StrongBox, else TEE) P-256 keys |
| Protocol | `src/homeNode/protocol.ts`, `words.ts`, `lan.ts`, `http1.ts`, `client.ts`, `pairing.ts` |
| Replica and queue | `src/homeNode/replica.ts`, `queue.ts`, `seal.ts` |
| Screens and state | `src/native/` (`HomeNodeApp.tsx` is the root) |
| Tests | `src/homeNode/*.test.ts`; `vectors.test.ts` checks the node's `vectors.json` |

## Keys

Pairing creates two P-256 keys that never leave the phone's hardware:

- the **device key** (`key`) signs every request. Using it needs Face ID,
  Touch ID, a fingerprint or the screen lock, and one check lasts 5 minutes;
- the **watch key** (`watchKey`) needs no check, so background checks and
  push wake-ups can sync. The node accepts it for `POST /remote/sync` only.

It also creates the X25519 discovery key that lets Tor find the admin onion.
The app asks for Face ID / the screen lock when it opens and after 5 minutes
in the background. Spending always asks for the wallet passphrase.

## Finding the node on home Wi-Fi

The app tries the LAN addresses the node reports in sync (and the ones in the
pairing QR) plus `openbazaar.local:8790`, takes the first that answers a
sealed probe within 1.5 s, and otherwise uses Tor. There is no mDNS browsing in
the app: the maintained React Native options predate the New Architecture.
iOS resolves `openbazaar.local` itself; Android only through the addresses
the node reports, so the first pairing on Android goes over Tor unless the QR
carries an address.

## Build

```sh
scripts/build-mobile-tor.sh ios aarch64-apple-ios-sim   # or aarch64-apple-ios
scripts/build-mobile-tor.sh android arm64-v8a x86_64    # needs the NDK (27.1)
cd apps/mobile && npm ci && npx expo prebuild
```

Then build `ios/OpenBazaar.xcworkspace` in Xcode, or run
`./gradlew assembleRelease` in `android/`. CI does the same in
`.github/workflows/mobile.yml`, without an Expo account or EAS.

## Credentials

None of these are in the repository. Each is set up once by whoever publishes
the app.

### Android APK signing

1. `keytool -genkeypair -v -keystore openbazaar-upload.jks -alias openbazaar -keyalg RSA -keysize 4096 -validity 10000`
2. Keep the keystore and its passwords offline as well: an APK signed with a
   new key cannot update an installed one.
3. Add the repository secrets `ANDROID_KEYSTORE_BASE64` (`base64 -i openbazaar-upload.jks`),
   `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` and `ANDROID_KEY_PASSWORD`.

A v* tag then attaches `OpenBazaar_<version>_android.apk` and its `.sha256`
to the release.

### App Store (or an EU alternative marketplace)

1. Join the Apple Developer Program and register the bundle ID
   `app.openbazaar.mobile` with the Push Notifications capability.
2. Create an Apple Distribution certificate, export it as a `.p12`, and an App
   Store provisioning profile for the bundle ID.
3. Add the secrets `APPLE_IOS_CERTIFICATE` (base64 of the `.p12`),
   `APPLE_IOS_CERTIFICATE_PASSWORD`, `APPLE_IOS_PROVISIONING_PROFILE` (base64)
   and `APPLE_TEAM_ID`. CI then builds a signed `.ipa` as an artifact.
4. Upload it with Transporter or `xcrun altool`, fill in the App Store Connect
   listing and the encryption declaration (the app declares exempt use), and
   submit. For an EU marketplace, accept the alternative terms addendum, then
   notarize the same build for that marketplace.

The App Store takes numeric versions only, so CI ships `0.1.0-alpha.1` as
`0.1.0` with the run number as the build number.

### Push (optional, off by default)

Push needs a push gateway: a small service that holds the APNs and FCM
credentials and forwards `POST /v1/wake { platform, token }` from the node
(`ob-node serve --push-gateway http://ONION`). Its operator sees push tokens
and when each wakes, never store data.

- **APNs**: in the developer account create an APNs auth key (`.p8`) and note
  its key ID and the team ID. The gateway signs with it; the app needs only the
  Push Notifications capability (`aps-environment`, added by the
  `expo-notifications` plugin) and an App Store or development-signed build.
- **FCM**: create a Firebase project, add the Android app
  `app.openbazaar.mobile`, download `google-services.json` to
  `apps/mobile/google-services.json` and set
  `"android": { "googleServicesFile": "./google-services.json" }` in `app.json`.
  The gateway uses a service account key of the same project (FCM HTTP v1).

Without these the push switch explains that this build cannot receive push.
Android still checks the node every 15 minutes or more over Tor with the
watch key and shows a generic notice for new orders; iOS syncs when opened.
