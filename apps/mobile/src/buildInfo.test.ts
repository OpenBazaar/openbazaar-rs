import assert from "node:assert/strict";
import { test } from "node:test";

import { buildMismatch, buildReport, parseNodeBuild, shellBuild, versionLine, type BuildInfo } from "./buildInfo.ts";

const shell: BuildInfo = {
  version: "0.1.0-alpha.1",
  commit: "a1b2c3d",
  dirty: false,
  buildDate: "2026-11-07T02:00:00Z",
  buildKind: "dev",
};
const node = { ...shell, buildDate: "2026-11-07T02:01:00Z", network: "testnet4" as const };

test("the same version and commit is not a mismatch, whatever the build date", () => {
  assert.equal(buildMismatch(shell, node), null);
  assert.equal(buildMismatch(shell, { ...node, dirty: true, buildKind: "ci" }), null);
});

test("a different version or commit is a mismatch that names both builds", () => {
  const version = buildMismatch(shell, { ...node, version: "0.0.0" });
  assert.ok(version?.includes("0.1.0-alpha.1 (a1b2c3d)"), version ?? "");
  assert.ok(version?.includes("node is 0.0.0 (a1b2c3d)"), version ?? "");
  const commit = buildMismatch(shell, { ...node, commit: "9f8e7d6", dirty: true });
  assert.ok(commit?.includes("node is 0.1.0-alpha.1 (9f8e7d6-dirty)"), commit ?? "");
});

test("an unknown commit or a missing side is not compared", () => {
  assert.equal(buildMismatch(shell, { ...node, commit: "unknown" }), null);
  assert.equal(buildMismatch({ ...shell, commit: "unknown" }, node), null);
  assert.notEqual(buildMismatch({ ...shell, commit: "unknown" }, { ...node, version: "0.2.0" }), null);
  assert.equal(buildMismatch(null, node), null);
  assert.equal(buildMismatch(shell, null), null);
});

test("the version line prefers the shell build and adds the node's network", () => {
  assert.equal(versionLine(shell, node), "OpenBazaar 0.1.0-alpha.1 · a1b2c3d · testnet4");
  assert.equal(versionLine(null, { ...node, dirty: true }), "OpenBazaar 0.1.0-alpha.1 · a1b2c3d-dirty · testnet4");
  assert.equal(versionLine(shell, null), "OpenBazaar 0.1.0-alpha.1 · a1b2c3d");
  assert.equal(versionLine(null, null), null);
  assert.match(buildReport(shell, node), /^OpenBazaar .*\nApp: 0\.1\.0-alpha\.1 a1b2c3d dev build .*\nNode: .* testnet4$/);
});

test("build info from the shell and node is checked before use", () => {
  assert.deepEqual(shellBuild({ __OB_SHELL_BUILD__: shell }), shell);
  assert.equal(shellBuild({}), null);
  assert.equal(shellBuild({ __OB_SHELL_BUILD__: { ...shell, dirty: "no" } }), null);
  assert.deepEqual(parseNodeBuild(node), node);
  assert.throws(() => parseNodeBuild({ ...node, network: "regtest" }));
  assert.throws(() => parseNodeBuild({ ...node, version: 1 }));
});
