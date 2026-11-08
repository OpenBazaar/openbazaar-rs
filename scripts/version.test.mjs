import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { getVersion, inheritingCrates, mismatches, setVersion, versions } from "./version.mjs";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");

/// A copy of just the files the version script reads.
function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "ob-version-"));
  const files = ["Cargo.toml", "Cargo.lock", ...versions(repo).map((item) => item.file.split(" ")[0])];
  const members = /members = \[([^\]]*)\]/.exec(readFileSync(join(repo, "Cargo.toml"), "utf8"))[1];
  for (const [, member] of members.matchAll(/"([^"]+)"/g)) files.push(join(member, "Cargo.toml"));
  for (const file of new Set(files)) cpSync(join(repo, file), join(dir, file));
  return dir;
}

test("every version in the repository matches the root Cargo.toml", () => {
  assert.deepEqual(mismatches(repo), []);
  assert.ok(inheritingCrates(repo).includes("ob-node"));
});

test("set writes the version everywhere and keeps the rest of each file", () => {
  const dir = fixture();
  try {
    const before = readFileSync(join(dir, "apps/desktop/src-tauri/tauri.conf.json"), "utf8");
    setVersion(dir, "9.8.7-rc.1");
    assert.equal(getVersion(dir), "9.8.7-rc.1");
    assert.deepEqual(mismatches(dir), []);
    assert.ok(versions(dir).every((item) => item.value === "9.8.7-rc.1"));
    const after = readFileSync(join(dir, "apps/desktop/src-tauri/tauri.conf.json"), "utf8");
    assert.equal(after, before.replace(`"version": "${getVersion(repo)}"`, `"version": "9.8.7-rc.1"`));
    const lock = readFileSync(join(dir, "Cargo.lock"), "utf8");
    assert.match(lock, /name = "ob-node"\nversion = "9\.8\.7-rc\.1"\n/);
    assert.doesNotMatch(lock, /name = "serde"\nversion = "9\.8\.7-rc\.1"/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("check names the file that disagrees", () => {
  const dir = fixture();
  try {
    const path = join(dir, "apps/mobile/app.json");
    const app = JSON.parse(readFileSync(path, "utf8"));
    app.expo.version = "0.0.1";
    writeFileSync(path, JSON.stringify(app, null, 2) + "\n");
    assert.deepEqual(mismatches(dir), [{ file: "apps/mobile/app.json expo.version", value: "0.0.1" }]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a Windows checkout with CRLF still reads and writes the version", () => {
  const dir = fixture();
  try {
    const files = ["Cargo.toml", "Cargo.lock", ...versions(dir).map((item) => item.file.split(" ")[0])];
    const members = /members = \[([^\]]*)\]/.exec(readFileSync(join(dir, "Cargo.toml"), "utf8"))[1];
    for (const [, member] of members.matchAll(/"([^"]+)"/g)) files.push(join(member, "Cargo.toml"));
    for (const file of new Set(files)) {
      const path = join(dir, file);
      writeFileSync(path, readFileSync(path, "utf8").replace(/\r?\n/g, "\r\n"));
    }
    assert.equal(getVersion(dir), getVersion(repo));
    setVersion(dir, "9.8.7-rc.1");
    assert.equal(getVersion(dir), "9.8.7-rc.1");
    assert.deepEqual(mismatches(dir), []);
    const cargo = readFileSync(join(dir, "Cargo.toml"), "utf8");
    assert.match(cargo, /\r\n\[workspace\.package\]\r\n/);
    assert.match(cargo, /version = "9\.8\.7-rc\.1"\r\n/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("set refuses a version that is not semver", () => {
  const dir = fixture();
  try {
    for (const bad of ["1.2", "v1.2.3", "1.2.3-", "latest"]) {
      assert.throws(() => setVersion(dir, bad), /not a semver version/);
    }
    assert.equal(getVersion(dir), getVersion(repo));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
