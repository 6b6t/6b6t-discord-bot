import type { Client, Snowflake } from "discord.js";
import config from "../config/config";
import {
  collectUserInfo,
  getAllLinkedUsers,
  type UserInfo,
  type UserLinkAndInfo,
} from "../utils/helpers";
import { mapWithConcurrency } from "../utils/map-with-concurrency";
import { buildRoleSyncPlan } from "./role-sync-plan";

const USER_INFO_CONCURRENCY = 8;
const USER_INFO_CACHE_TTL_MS = 5 * 60_000;
const userInfoCache = new Map<string, { value: UserInfo; expiresAt: number }>();

const roles: Record<
  string,
  {
    id: Snowflake;
    predicate: (check: UserInfo) => boolean;
  }
> = {
  prime: {
    id: "1268337190144835718",
    predicate: (info) => info.topRank === "prime",
  },
  primeultra: {
    id: "1325147393372586054",
    predicate: (info) => info.topRank === "primeultra",
  },
  elite: {
    id: "1268337279898878013",
    predicate: (info) => info.topRank === "elite",
  },
  eliteultra: {
    id: "1325147417322192927",
    predicate: (info) => info.topRank === "eliteultra",
  },
  apex: {
    id: "1268345919003430942",
    predicate: (info) => info.topRank === "apex",
  },
  legend: {
    id: "1349026308390391839",
    predicate: (info) => info.topRank === "legend",
  },
  "2022": {
    id: "1349065372313321514",
    predicate: (info) => info.firstJoinYear <= 2022,
  },
  "2023": {
    id: "1349065403477004480",
    predicate: (info) => info.firstJoinYear === 2023,
  },
  "2024": {
    id: "1349065422065893516",
    predicate: (info) => info.firstJoinYear === 2024,
  },
  "2025": {
    id: "1349065443650043955",
    predicate: (info) => info.firstJoinYear === 2025,
  },
  "2026": {
    id: "1453085388930416702",
    predicate: (info) => info.firstJoinYear === 2026,
  },
};

const LINKED_ROLE_ID = "1325507259307921428";

export const sync = async (client: Client) => {
  const linkLog = (step: string, message: string) =>
    console.log(`[LinkedRoles][${step}] ${message}`);

  linkLog("Bootstrap", "Fetching linked users");
  const linkedUsers = await getAllLinkedUsers();
  linkLog("Bootstrap", `Fetched ${linkedUsers.length} linked users`);

  const linkedUserIds = new Set(linkedUsers.map((user) => user.discordId));

  linkLog("Guild", "Fetching guild info");
  const guild = await client.guilds.fetch(config.guildId);
  if (!guild) return console.error("Guild not found");

  try {
    await guild.members.fetch();
  } catch (error) {
    if (
      error instanceof Error &&
      error.name === "GatewayRateLimitError" &&
      "data" in error
    ) {
      const retryAfter = (error as { data: { retry_after: number } }).data
        .retry_after;
      linkLog(
        "Guild",
        `Rate limited fetching members, retrying after ${retryAfter}s`,
      );
      await new Promise((resolve) =>
        setTimeout(resolve, retryAfter * 1000 + 100),
      );
      await guild.members.fetch();
    } else {
      throw error;
    }
  }

  // Build bypass set: members having the manually managed role
  const bypassMembers = new Set<string>();
  try {
    const rid = config.manuallyManagedRoleId;
    if (rid) {
      const role = await guild.roles.fetch(rid);
      role?.members.forEach((m) => {
        bypassMembers.add(m.id);
      });
    }
  } catch (e) {
    console.error("Failed to resolve manually managed role(s):", e);
  }
  linkLog(
    "Bypass",
    `Bypassing ${bypassMembers.size} manually managed member(s)`,
  );

  const syncRole = async (
    key: string,
    id: Snowflake,
    allowedUserIds: string[],
    shouldRemove: (memberId: string) => boolean,
  ) => {
    linkLog(key, `Preparing role ${id}`);
    const role = await guild.roles.fetch(id);
    if (!role) {
      linkLog(key, "Role not found, skipping");
      return;
    }

    if (!role.editable) {
      console.error(
        `[LinkedRoles][${key}] Role ${id} is not editable. Move the bot role above it and grant Manage Roles.`,
      );
      return;
    }

    const removableMemberIds = new Set(
      role.members
        .map((member) => member.id)
        .filter((memberId) => shouldRemove(memberId)),
    );
    const plan = buildRoleSyncPlan({
      allowedUserIds,
      currentMemberIds: role.members.map((member) => member.id),
      bypassMemberIds: bypassMembers,
      removableMemberIds,
    });
    const { add: membersToAdd, remove: membersToRemove } = plan;

    linkLog(
      key,
      `Allowed=${plan.allowed.size}, current=${role.members.size}, add=${membersToAdd.length}, remove=${membersToRemove.length}`,
    );

    let addSuccessCount = 0;
    let addMissingCount = 0;
    let addFailureCount = 0;
    for (const memberId of membersToAdd) {
      const member = guild.members.cache.get(memberId);
      if (!member) {
        addMissingCount++;
        continue;
      }

      try {
        await member.roles.add(role, `6b6t ${key} role sync`);
        addSuccessCount++;
      } catch (error) {
        addFailureCount++;
        console.error(
          `[LinkedRoles][${key}] Failed to add role ${id} to ${memberId}:`,
          error,
        );
      }
    }

    linkLog(
      key,
      `Add summary: tried=${membersToAdd.length}, succeeded=${addSuccessCount}, missing=${addMissingCount}, failed=${addFailureCount}`,
    );

    let removeSuccessCount = 0;
    let removeMissingCount = 0;
    let removeFailureCount = 0;
    for (const memberId of membersToRemove) {
      const member = guild.members.cache.get(memberId);
      if (!member) {
        removeMissingCount++;
        continue;
      }

      try {
        await member.roles.remove(role, `6b6t ${key} role sync`);
        removeSuccessCount++;
      } catch (error) {
        removeFailureCount++;
        console.error(
          `[LinkedRoles][${key}] Failed to remove role ${id} from ${memberId}:`,
          error,
        );
      }
    }

    linkLog(
      key,
      `Remove summary: tried=${membersToRemove.length}, succeeded=${removeSuccessCount}, missing=${removeMissingCount}, failed=${removeFailureCount}`,
    );
  };

  linkLog("Assign", "Assigning guild roles");
  await syncRole(
    "linked",
    LINKED_ROLE_ID,
    [...linkedUserIds],
    (memberId) => !linkedUserIds.has(memberId),
  );

  linkLog("CollectInfo", "Collecting user info");
  const collectionResults = await mapWithConcurrency(
    linkedUsers,
    USER_INFO_CONCURRENCY,
    async (linkedUser) => {
      const cached = userInfoCache.get(linkedUser.minecraftUuid);
      if (cached && cached.expiresAt > Date.now()) {
        return {
          status: "resolved" as const,
          user: { ...linkedUser, ...cached.value } satisfies UserLinkAndInfo,
        };
      }

      if (cached) {
        userInfoCache.delete(linkedUser.minecraftUuid);
      }

      try {
        const userInfo = await collectUserInfo(linkedUser.minecraftUuid);
        if (userInfo === null) {
          return { status: "unresolved" as const, linkedUser };
        }

        userInfoCache.set(linkedUser.minecraftUuid, {
          value: userInfo,
          expiresAt: Date.now() + USER_INFO_CACHE_TTL_MS,
        });
        return {
          status: "resolved" as const,
          user: { ...linkedUser, ...userInfo } satisfies UserLinkAndInfo,
        };
      } catch (error) {
        return { status: "failed" as const, linkedUser, error };
      }
    },
  );

  const userLinksAndInfos = collectionResults.flatMap((result) =>
    result.status === "resolved" ? [result.user] : [],
  );
  const unresolvedCount = collectionResults.filter(
    (result) => result.status === "unresolved",
  ).length;
  const failedResults = collectionResults.filter(
    (result) => result.status === "failed",
  );

  for (const result of failedResults.slice(0, 5)) {
    console.error(
      `[LinkedRoles][CollectInfo] Failed to resolve ${result.linkedUser.discordId}:`,
      result.error,
    );
  }
  if (failedResults.length > 5) {
    linkLog(
      "CollectInfo",
      `Suppressed ${failedResults.length - 5} additional lookup errors`,
    );
  }

  linkLog(
    "CollectInfo",
    `Resolved=${userLinksAndInfos.length}, unresolved=${unresolvedCount}, failed=${failedResults.length}, concurrency=${USER_INFO_CONCURRENCY}`,
  );

  const resolvedUserIds = new Set(
    userLinksAndInfos.map((user) => user.discordId),
  );

  for (const [key, { id, predicate }] of Object.entries(roles)) {
    const allowedUserIds = userLinksAndInfos
      .filter((user) => predicate(user))
      .map((user) => user.discordId);

    await syncRole(
      key,
      id,
      allowedUserIds,
      (memberId) =>
        !linkedUserIds.has(memberId) || resolvedUserIds.has(memberId),
    );
  }

  linkLog("Assign", "Linked role sync complete");
};
