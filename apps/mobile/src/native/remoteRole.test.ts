import assert from "node:assert/strict";
import { test } from "node:test";

import { remoteRoleNotice } from "./remoteRole.ts";

test("the phone says it is a remote, whether or not the home node has answered", () => {
  assert.match(remoteRoleNotice(false), /remote for your home node/);
  assert.match(remoteRoleNotice(false), /until that node answers/);
  assert.match(remoteRoleNotice(true), /stop when that node is off/);
  assert.doesNotMatch(remoteRoleNotice(true), /full node|push gateway/i);
});
