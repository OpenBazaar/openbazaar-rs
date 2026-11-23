import assert from "node:assert/strict";
import { test } from "node:test";

import { remoteRoleNotice } from "./remoteRole.ts";

test("the phone says it is a remote, and that a store address opens without that node", () => {
  assert.match(remoteRoleNotice(false), /remote for your home node/);
  assert.match(remoteRoleNotice(false), /store address/);
  assert.match(remoteRoleNotice(false), /until that node answers/);
  assert.match(remoteRoleNotice(true), /store address/);
  assert.match(remoteRoleNotice(true), /Paying needs the node/);
  assert.doesNotMatch(remoteRoleNotice(false), /full node|push gateway/i);
  assert.doesNotMatch(remoteRoleNotice(true), /full node|push gateway/i);
});
