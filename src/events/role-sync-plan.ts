export type RoleSyncPlanInput = {
  allowedUserIds: Iterable<string>;
  currentMemberIds: Iterable<string>;
  bypassMemberIds: ReadonlySet<string>;
  removableMemberIds: ReadonlySet<string>;
};

export type RoleSyncPlan = {
  allowed: Set<string>;
  add: string[];
  remove: string[];
};

export function buildRoleSyncPlan({
  allowedUserIds,
  currentMemberIds,
  bypassMemberIds,
  removableMemberIds,
}: RoleSyncPlanInput): RoleSyncPlan {
  const allowed = new Set(
    [...allowedUserIds].filter((id) => !bypassMemberIds.has(id)),
  );
  const current = new Set(
    [...currentMemberIds].filter((id) => !bypassMemberIds.has(id)),
  );

  return {
    allowed,
    add: [...allowed].filter((id) => !current.has(id)),
    remove: [...current].filter(
      (id) => !allowed.has(id) && removableMemberIds.has(id),
    ),
  };
}
