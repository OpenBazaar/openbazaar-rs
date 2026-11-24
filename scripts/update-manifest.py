#!/usr/bin/env python3
"""Write the desktop app's update manifest (latest.json) for one release.

The release job runs this over its folder of installers after the desktop
builds signed their update bundles (docs/updates.md). Every platform the app
updates on must be there with its minisign signature, or no manifest is
written: a manifest that lists only some platforms would leave the rest
silently stuck on an old version.

    update-manifest.py --version 0.1.0-alpha.15 --repo OpenBazaar/openbazaar-rs --out latest.json DIR

The keys are Tauri's updater targets. A universal macOS bundle serves both
Mac architectures. Linux lists only the AppImage: a .deb install cannot
replace itself, so it finds no entry and tells the user to download instead.
"""
import argparse
import datetime
import json
import sys
from pathlib import Path

# (Tauri target keys, file name pattern with {v} for the version)
PLATFORMS = [
    (("darwin-aarch64", "darwin-x86_64", "darwin-aarch64-app", "darwin-x86_64-app"), "OpenBazaar_{v}_universal.app.tar.gz"),
    (("windows-x86_64", "windows-x86_64-nsis"), "OpenBazaar_{v}_x64-setup.exe"),
    (("linux-x86_64-appimage",), "OpenBazaar_{v}_amd64.AppImage"),
]


def manifest(folder: Path, version: str, repo: str, now: datetime.datetime) -> dict:
    base = f"https://github.com/{repo}/releases/download/v{version}"
    platforms = {}
    missing = []
    for keys, pattern in PLATFORMS:
        name = pattern.format(v=version)
        bundle = folder / name
        sig = folder / f"{name}.sig"
        if not bundle.is_file() or not sig.is_file():
            missing.append(name if not bundle.is_file() else f"{name}.sig")
            continue
        signature = sig.read_text().strip()
        if not signature:
            missing.append(f"{name}.sig (empty)")
            continue
        for key in keys:
            platforms[key] = {"signature": signature, "url": f"{base}/{name}"}
    if missing:
        raise SystemExit("No manifest: missing " + ", ".join(missing))
    return {
        "version": version,
        "notes": f"OpenBazaar {version}. Release notes: https://github.com/{repo}/releases/tag/v{version}",
        "pub_date": now.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--version", required=True, help="semver without the v, e.g. 0.1.0-alpha.15")
    parser.add_argument("--repo", required=True, help="owner/name on GitHub")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("folder", type=Path)
    args = parser.parse_args(argv)
    version = args.version.removeprefix("v")
    data = manifest(args.folder, version, args.repo, datetime.datetime.now(datetime.timezone.utc))
    args.out.write_text(json.dumps(data, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
