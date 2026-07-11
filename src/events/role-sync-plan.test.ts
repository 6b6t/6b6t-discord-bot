import assert from "node:assert/strict";
import test from "node:test";
import { buildRoleSyncPlan } from "./role-sync-plan";

const EMPTY_SET = new Set<string>();

test("adds every mapped guild member missing the linked role", () => {
  const plan = buildRoleSyncPlan({
    allowedUserIds: ["linked", "already-linked"],
    currentMemberIds: ["already-linked"],
    bypassMemberIds: EMPTY_SET,
    removableMemberIds: new Set(["already-linked"]),
  });

  assert.deepEqual(plan.add, ["linked"]);
  assert.deepEqual(plan.remove, []);
});

test("removes the linked role after the mapping is deleted", () => {
  const plan = buildRoleSyncPlan({
    allowedUserIds: [],
    currentMemberIds: ["unlinked"],
    bypassMemberIds: EMPTY_SET,
    removableMemberIds: new Set(["unlinked"]),
  });

  assert.deepEqual(plan.add, []);
  assert.deepEqual(plan.remove, ["unlinked"]);
});

test("preserves rank roles when linked player data is unresolved", () => {
  const plan = buildRoleSyncPlan({
    allowedUserIds: [],
    currentMemberIds: ["temporarily-unresolved"],
    bypassMemberIds: EMPTY_SET,
    removableMemberIds: EMPTY_SET,
  });

  assert.deepEqual(plan.remove, []);
});

test("removes an old rank after an authoritative rank change", () => {
  const plan = buildRoleSyncPlan({
    allowedUserIds: [],
    currentMemberIds: ["changed-rank"],
    bypassMemberIds: EMPTY_SET,
    removableMemberIds: new Set(["changed-rank"]),
  });

  assert.deepEqual(plan.remove, ["changed-rank"]);
});

test("never changes manually managed members", () => {
  const plan = buildRoleSyncPlan({
    allowedUserIds: ["bypassed-add"],
    currentMemberIds: ["bypassed-remove"],
    bypassMemberIds: new Set(["bypassed-add", "bypassed-remove"]),
    removableMemberIds: new Set(["bypassed-remove"]),
  });

  assert.deepEqual(plan.add, []);
  assert.deepEqual(plan.remove, []);
});

test("deduplicates mappings before planning role additions", () => {
  const plan = buildRoleSyncPlan({
    allowedUserIds: ["linked", "linked"],
    currentMemberIds: [],
    bypassMemberIds: EMPTY_SET,
    removableMemberIds: EMPTY_SET,
  });

  assert.deepEqual(plan.add, ["linked"]);
});
