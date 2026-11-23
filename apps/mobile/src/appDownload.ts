// The app a buyer downloads so they can pay. This page does not take a card
// and does not pay the seller. The files are the v0.1.0-alpha.12 release.

export const APP_VERSION = "0.1.0-alpha.12";
const TAG = `v${APP_VERSION}`;
const BASE = `https://github.com/OpenBazaar/openbazaar-rs/releases/download/${TAG}`;

export const RELEASE_PAGE = `https://github.com/OpenBazaar/openbazaar-rs/releases/tag/${TAG}`;

export type AppDownload = { href: string; label: string };

/** The release file for this browser, or the release page when there is no matching build. */
export function appDownload(userAgent: string): AppDownload {
  const ua = userAgent.toLowerCase();
  if (ua.includes("android")) {
    return { href: `${BASE}/OpenBazaar_${APP_VERSION}_android.apk`, label: "Android app" };
  }
  if (ua.includes("iphone") || ua.includes("ipad") || ua.includes("ipod")) {
    return { href: RELEASE_PAGE, label: "v0.1.0-alpha.12 release" };
  }
  if (ua.includes("mac os") || ua.includes("macintosh")) {
    return { href: `${BASE}/OpenBazaar_${APP_VERSION}_universal.dmg`, label: "Mac app" };
  }
  if (ua.includes("windows")) {
    return { href: `${BASE}/OpenBazaar_${APP_VERSION}_x64-setup.exe`, label: "Windows app" };
  }
  if (ua.includes("linux")) {
    return { href: `${BASE}/OpenBazaar_${APP_VERSION}_amd64.AppImage`, label: "Linux app" };
  }
  return { href: RELEASE_PAGE, label: "v0.1.0-alpha.12 release" };
}
