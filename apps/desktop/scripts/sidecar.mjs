// Build ob-node for the Tauri target and place it where `bundle.externalBin`
// expects it: src-tauri/binaries/ob-node-<target-triple>[.exe].
//
// Tauri sets TAURI_ENV_TARGET_TRIPLE when it runs beforeBuildCommand. Run by
// hand, the script builds for the host. universal-apple-darwin builds both
// macOS architectures and joins them with lipo.
import { execFileSync } from "node:child_process";
import { copyFileSync, chmodSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const desktop = dirname(dirname(fileURLToPath(import.meta.url)));
const root = join(desktop, "..", "..");
const binaries = join(desktop, "src-tauri", "binaries");

function run(command, args) {
  execFileSync(command, args, { stdio: "inherit", cwd: root });
}

function hostTriple() {
  const info = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const line = info.split("\n").find((entry) => entry.startsWith("host: "));
  if (!line) throw new Error("rustc -vV did not report a host triple");
  return line.slice("host: ".length).trim();
}

function build(triple) {
  run("cargo", ["build", "--release", "--locked", "-p", "ob-node", "--target", triple]);
  const exe = triple.includes("windows") ? ".exe" : "";
  return join(root, "target", triple, "release", `ob-node${exe}`);
}

const triple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTriple();
const exe = triple.includes("windows") ? ".exe" : "";
const destination = join(binaries, `ob-node-${triple}${exe}`);
mkdirSync(binaries, { recursive: true });

if (triple === "universal-apple-darwin") {
  const arm = build("aarch64-apple-darwin");
  const intel = build("x86_64-apple-darwin");
  execFileSync("lipo", ["-create", "-output", destination, arm, intel], { stdio: "inherit" });
} else {
  copyFileSync(build(triple), destination);
}
if (!exe) chmodSync(destination, 0o755);
console.log(`sidecar ready: ${destination}`);
