#!/usr/bin/env node
// One version for the whole app. The source of truth is `version` under
// [workspace.package] in the root Cargo.toml; ob-node and the other crates
// that say `version.workspace = true` inherit it. Every other place that
// carries the app version must match it:
//
//   node scripts/version.mjs              print the version
//   node scripts/version.mjs check        fail if any file disagrees
//   node scripts/version.mjs set 1.2.3    write the version everywhere
//
// Files are edited in place so their formatting is kept, then re-read to
// confirm only the version changed.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

const JSON_FILES = [
  { path: "apps/desktop/src-tauri/tauri.conf.json", keys: [["version"]] },
  { path: "apps/desktop/package.json", keys: [["version"]] },
  { path: "apps/desktop/package-lock.json", keys: [["version"], ["packages", "", "version"]] },
  { path: "apps/mobile/package.json", keys: [["version"]] },
  { path: "apps/mobile/package-lock.json", keys: [["version"], ["packages", "", "version"]] },
  { path: "apps/mobile/app.json", keys: [["expo", "version"]] },
];
const DESKTOP_CRATE = { path: "apps/desktop/src-tauri/Cargo.toml", section: "package" };
const DESKTOP_LOCK = { path: "apps/desktop/src-tauri/Cargo.lock", packages: ["openbazaar-desktop"] };

const read = (root, path) => readFileSync(join(root, path), "utf8");

function tomlSection(text, header) {
  const start = text.indexOf(`\n[${header}]\n`);
  if (start < 0) throw new Error(`[${header}] is missing`);
  const body = start + header.length + 4;
  const next = text.slice(body).search(/^\[/m);
  return [body, next < 0 ? text.length : body + next];
}

function tomlVersion(text, header) {
  const [from, to] = tomlSection(text, header);
  const match = /^version = "([^"]*)"$/m.exec(text.slice(from, to));
  if (!match) throw new Error(`[${header}] has no version`);
  return { value: match[1], at: from + match.index, length: match[0].length };
}

function setTomlVersion(text, header, version) {
  const found = tomlVersion(text, header);
  return text.slice(0, found.at) + `version = "${version}"` + text.slice(found.at + found.length);
}

/// Path packages (no `source` line) in a Cargo.lock, by name.
function lockEntries(text, names) {
  const entries = [];
  const block = /\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]*)"\n(source = )?/g;
  for (const match of text.matchAll(block)) {
    if (names.includes(match[1]) && !match[3]) entries.push({ name: match[1], value: match[2], match });
  }
  return entries;
}

function setLockVersions(text, names, version) {
  let out = text;
  for (const { name, match } of lockEntries(text, names).reverse()) {
    const replaced = `[[package]]\nname = "${name}"\nversion = "${version}"\n`;
    out = out.slice(0, match.index) + replaced + out.slice(match.index + match[0].length - (match[3] ?? "").length);
  }
  return out;
}

const getPath = (value, keys) => keys.reduce((node, key) => (node == null ? undefined : node[key]), value);

/// Replace the string at `keys` without reformatting the file. Each key is
/// looked up after the previous one, one indent level deeper.
function setJsonString(text, keys, version) {
  let from = 0;
  let at = -1;
  keys.forEach((key, depth) => {
    const needle = `\n${"  ".repeat(depth + 1)}${JSON.stringify(key)}: `;
    at = text.indexOf(needle, from);
    if (at < 0) throw new Error(`${keys.join(".")} not found`);
    from = at + needle.length;
  });
  const value = /^"[^"]*"/.exec(text.slice(from));
  if (!value) throw new Error(`${keys.join(".")} is not a string`);
  return text.slice(0, from) + JSON.stringify(version) + text.slice(from + value[0].length);
}

/// Workspace members whose Cargo.toml says `version.workspace = true`.
export function inheritingCrates(root) {
  const cargo = read(root, "Cargo.toml");
  const members = /members = \[([^\]]*)\]/.exec(cargo);
  if (!members) throw new Error("Cargo.toml has no workspace members");
  return [...members[1].matchAll(/"([^"]+)"/g)]
    .map(([, dir]) => read(root, join(dir, "Cargo.toml")))
    .filter((manifest) => /^version\.workspace = true$/m.test(manifest))
    .map((manifest) => /^name = "([^"]+)"$/m.exec(manifest)[1]);
}

export function getVersion(root) {
  return tomlVersion(read(root, "Cargo.toml"), "workspace.package").value;
}

/// Every place the version lives, as `{ file, value }`.
export function versions(root) {
  const found = [{ file: "Cargo.toml [workspace.package]", value: getVersion(root) }];
  const crates = inheritingCrates(root);
  const lock = lockEntries(read(root, "Cargo.lock"), crates);
  for (const name of crates) {
    const entry = lock.find((item) => item.name === name);
    found.push({ file: `Cargo.lock ${name}`, value: entry?.value });
  }
  found.push({ file: `${DESKTOP_CRATE.path} [package]`, value: tomlVersion(read(root, DESKTOP_CRATE.path), DESKTOP_CRATE.section).value });
  for (const name of DESKTOP_LOCK.packages) {
    const entry = lockEntries(read(root, DESKTOP_LOCK.path), [name])[0];
    found.push({ file: `${DESKTOP_LOCK.path} ${name}`, value: entry?.value });
  }
  for (const { path, keys } of JSON_FILES) {
    const json = JSON.parse(read(root, path));
    for (const key of keys) found.push({ file: `${path} ${key.map((k) => k || '""').join(".")}`, value: getPath(json, key) });
  }
  return found;
}

/// The files that disagree with the root Cargo.toml.
export function mismatches(root) {
  const expected = getVersion(root);
  return versions(root).filter((item) => item.value !== expected);
}

export function setVersion(root, version) {
  if (!SEMVER.test(version)) throw new Error(`${version} is not a semver version`);
  const crates = inheritingCrates(root);
  const edits = [
    ["Cargo.toml", (text) => setTomlVersion(text, "workspace.package", version)],
    ["Cargo.lock", (text) => setLockVersions(text, crates, version)],
    [DESKTOP_CRATE.path, (text) => setTomlVersion(text, DESKTOP_CRATE.section, version)],
    [DESKTOP_LOCK.path, (text) => setLockVersions(text, DESKTOP_LOCK.packages, version)],
    ...JSON_FILES.map(({ path, keys }) => [
      path,
      (text) => {
        const updated = keys.reduce((out, key) => setJsonString(out, key, version), text);
        const expected = JSON.parse(text);
        for (const key of keys) getPath(expected, key.slice(0, -1))[key.at(-1)] = version;
        if (JSON.stringify(JSON.parse(updated)) !== JSON.stringify(expected)) {
          throw new Error(`${path}: editing the version changed something else`);
        }
        return updated;
      },
    ]),
  ];
  for (const [path, edit] of edits) writeFileSync(join(root, path), edit(read(root, path)));
  const left = mismatches(root);
  if (left.length) throw new Error(`still out of date: ${left.map((item) => item.file).join(", ")}`);
}

function main([command = "get", value]) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  if (command === "get") {
    console.log(getVersion(root));
  } else if (command === "check") {
    const expected = getVersion(root);
    const bad = mismatches(root);
    for (const item of bad) console.error(`${item.file}: ${item.value ?? "(missing)"}, expected ${expected}`);
    if (bad.length) {
      console.error(`Run: node scripts/version.mjs set ${expected}`);
      process.exit(1);
    }
    console.log(`version ${expected} in all ${versions(root).length} places`);
  } else if (command === "set" && value) {
    setVersion(root, value.replace(/^v/, ""));
    console.log(`version set to ${getVersion(root)}`);
  } else {
    console.error("usage: node scripts/version.mjs [get | check | set VERSION]");
    process.exit(2);
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    main(process.argv.slice(2));
  } catch (err) {
    console.error(`version: ${err.message}`);
    process.exit(1);
  }
}
