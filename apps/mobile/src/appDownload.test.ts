import assert from "node:assert/strict";
import { test } from "node:test";

import { APP_VERSION, appDownload, RELEASE_PAGE } from "./appDownload.ts";

test("each platform gets a file from the current release", () => {
  const mac = appDownload("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)");
  assert.equal(mac.href, `https://github.com/OpenBazaar/openbazaar-rs/releases/download/v${APP_VERSION}/OpenBazaar_${APP_VERSION}_universal.dmg`);
  assert.equal(mac.label, "Mac app");
  assert.match(appDownload("Mozilla/5.0 (Windows NT 10.0)").href, /x64-setup\.exe$/);
  assert.match(appDownload("Mozilla/5.0 (X11; Linux x86_64)").href, /AppImage$/);
  assert.match(appDownload("Mozilla/5.0 (Linux; Android 14)").href, /android\.apk$/);
  assert.equal(appDownload("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)").href, RELEASE_PAGE);
  assert.equal(appDownload("").href, RELEASE_PAGE);
});
