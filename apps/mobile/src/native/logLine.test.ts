import assert from "node:assert/strict";
import { test } from "node:test";

import { logLine } from "./logLine.ts";

test("a device log line is one line and never holds a whole onion address", () => {
  const onion = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid";
  const line = logLine(`pairing: try 2 ended after 61s: could not reach\n${onion}.onion; asking again`);
  assert.equal(line, "ob-app: pairing: try 2 ended after 61s: could not reach 2gzyxa…3wid.onion; asking again");
  assert.ok(!line.includes(onion));
  assert.equal(logLine("x".repeat(500)).length, "ob-app: ".length + 300);
});
