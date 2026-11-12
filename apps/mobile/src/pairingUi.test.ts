import assert from "node:assert/strict";
import { test } from "node:test";

import {
  browsersCardCopy,
  pairingBanner,
  pairingDialog,
  pairingKind,
  pairingOrdersText,
  pairingWalletPrompt,
} from "./pairingUi.ts";

test("loopback copy offers Continue and does not mention the terminal", () => {
  const kind = pairingKind(true);
  const dialog = pairingDialog(kind);
  const banner = pairingBanner(kind);
  assert.equal(dialog.action, "Continue on this computer");
  assert.equal(banner.action, "Continue on this computer");
  assert.match(dialog.body, /this computer/);
  assert.doesNotMatch(dialog.body, /terminal/i);
  assert.doesNotMatch(banner.text, /terminal/i);
  assert.match(pairingOrdersText(kind), /Continue on this computer/);
  assert.equal(pairingWalletPrompt(kind).action, "Continue on this computer");
});

test("a remote browser still types a pairing code, not from a terminal", () => {
  const kind = pairingKind(false);
  const dialog = pairingDialog(kind);
  const banner = pairingBanner(kind);
  assert.equal(dialog.action, "Pair browser");
  assert.equal(banner.action, "Enter pairing code");
  assert.match(dialog.body, /pairing code from your node/);
  assert.doesNotMatch(dialog.body, /terminal/i);
  assert.match(pairingOrdersText(kind), /Type its pairing code/);
});

test("Settings mint copy shows the code on the page", () => {
  const copy = browsersCardCopy();
  assert.equal(copy.pageTitle, "Browsers");
  assert.equal(copy.action, "Create pairing code");
  assert.match(copy.shown, /5 minutes/);
  assert.match(copy.shown, /other browser/);
  assert.doesNotMatch(copy.shown, /terminal/i);
});
