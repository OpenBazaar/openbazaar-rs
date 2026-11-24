# Updates

How each OpenBazaar build gets a new version without the user deleting and reinstalling it. The desktop app does this today; the rest is the plan, in order.

## Rules for every platform

- **Every update is a full, signed release.** No over-the-air JavaScript or other code pushes. The app's code changes only when the user installs a release that the release key signed.
- **One manifest per channel.** The release job writes `latest.json` for the version it published and uploads it to a fixed release tag for that version's channel: `updates-alpha`, then `updates-beta` and `updates-stable` when those exist. The manifest names each platform's file on the versioned release and carries its signature. Clients read only their own channel.
- **The key is the trust root, not GitHub.** Desktop update bundles are signed with a minisign key whose secret half is a repository secret. The app ships the public half and refuses any download that does not verify, so a hijacked GitHub account, release, or mirror cannot push code to users.
- **Checks go over Tor.** A check tells whoever serves the manifest that this address runs OpenBazaar and which version. The desktop app sends the check through the node's built-in Tor, and does not check at all while that Tor is not up.
- **Ask, then install.** The app checks on its own, but it only downloads and installs when the user says so.
- **Nothing is lost.** Updates replace program files only. The data folder (keys, wallet, store, orders, escrows) stays where it is, and the node is built to restart at any point of an order or escrow.

## Desktop (macOS, Windows, Linux AppImage) — done

The shell uses `tauri-plugin-updater` from Rust only (`apps/desktop/src-tauri/src/updates.rs`). The shop page is served by the node over loopback and has no way to start an install.

- A minute after launch, and every 12 hours after that, the app reads the channel manifest through the node's Tor proxy (the address `ob-node serve --tor` writes to its log).
- When a newer version is listed, a dialog names both versions and the release notes, with **Install and restart** or **Later**. Later keeps that version from being offered again until the next launch.
- Install downloads while the node keeps running, verifies the signature, stops the node (so its binary can be replaced), installs, and relaunches. Windows runs the NSIS installer in passive mode. If install fails, the app says why and restarts on the version it had.
- Menu bar: **Check for updates…** checks now, and **Check for updates automatically** turns the schedule off or on (a file named `updates-off` in the app data folder remembers off).
- A build without an updater key (forks, local builds, releases made before the key existed) never checks, and "Check for updates…" points to the releases page.

Bundles: macOS is the universal `.app.tar.gz` for both architectures, Windows is the `-setup.exe`, Linux is the `.AppImage`. A `.deb` install cannot replace itself, so the manifest has no entry for it, and a check from it says to download the new version.

### One-time setup

1. Make the key pair on a trusted machine and keep the secret half offline as well:

   ```
   cd apps/desktop
   npx tauri signer generate -w ~/.tauri/openbazaar-updater.key
   ```

   Give it a password.

2. Put the **public** key (the `.pub` file's contents) in `apps/desktop/src-tauri/tauri.conf.json` at `plugins.updater.pubkey`, replacing `UPDATER_PUBLIC_KEY_NOT_SET`, and merge that.
3. Add repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the secret key file's contents) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Add the macOS signing and notarization secrets listed at the top of `.github/workflows/release.yml`. Updates install without them, but an unnotarized app is what macOS calls "damaged".

From the next tag on, `release.yml` signs the update bundles, attaches them and their `.sig` files to the release, writes `latest.json` with `scripts/update-manifest.py`, and points the channel's `updates-<channel>` release at it once every file is uploaded. A release run that has the secret but no public key in the config fails rather than ship bundles no app can verify.

The first version with this code still has to be installed by hand once. Every later one is offered in the app.

**Losing the secret key** means installed apps can never verify another update: users would reinstall once to get a build with a new public key. Keep a backup.

## Android — next

- Set `ANDROID_KEYSTORE_BASE64` and the other keystore secrets in `mobile.yml` first. Android installs an update only when it is signed with the same key as the installed APK, and an APK signed with a key made for one CI run can never be updated; phones that have one need one last reinstall.
- The app reads the same channel manifest (an `android` entry: APK URL, SHA-256, signing-certificate digest) over its Tor, asks, downloads, checks the hash and the certificate, and hands the APK to the system installer (`REQUEST_INSTALL_PACKAGES`).
- Later: F-Droid or Obtainium for people who prefer a store to in-app updates.

## iOS

Apple allows updates only through the App Store or TestFlight. The app can read the manifest and say a new version is out, with a link to the store; it cannot install anything itself.

## Linux .deb

An "update available" notice with a download link, from the same manifest. A signed apt repository can replace that later.

## Home node (`ob-node` tarball, systemd, launchd, Docker)

- `ob-node update [--check]`: read the channel manifest through `--proxy` or `--tor`, verify the tarball's signature with the release key built into the binary, swap the binary with the old one kept beside it, and let systemd or launchd restart the service.
- Docker: pull the new `ghcr.io/openbazaar/ob-node:<version>` tag.
- Railway: redeploy from the release tag (a step in `release.yml` is planned).

## Data folder versions (with the home-node work)

Each data folder gets a schema version. A node refuses to open a folder newer than it understands instead of guessing, which makes rolling back to the previous binary safe.
