import type { Client, Snowflake } from "discord.js";
import config from "../config/config";
import {
  collectUserInfo,
  getAllLinkedUsers,
  type UserInfo,
  type UserLinkAndInfo,
} from "../utils/helpers";

const roles: Record<
  string,
  {
    id: Snowflake;
    predicate: (check: UserInfo) => boolean;
  }
> = {
  linked: {
    id: "1325507259307921428",
    predicate: () => true,
  },
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

export const sync = async (client: Client) => {
  const linkLog = (step: string, message: string) =>
    console.log(`[LinkedRoles][${step}] ${message}`);

  linkLog("Bootstrap", "Fetching linked users");
  const linkedUsers = await getAllLinkedUsers();
  linkLog("Bootstrap", `Fetched ${linkedUsers.length} linked users`);

  linkLog("CollectInfo", "Collecting user info");
  const userLinksAndInfos: UserLinkAndInfo[] = (
    await Promise.all(
      linkedUsers.map(async (linkedUser) => {
        const userInfo = await collectUserInfo(linkedUser.minecraftUuid);
        if (userInfo === null) {
          linkLog(
            "CollectInfo",
            `Skipping ${linkedUser.discordId}: unable to resolve Minecraft data`,
          );
          return null;
        }

        const result: UserLinkAndInfo = {
          ...linkedUser,
          ...userInfo,
        };
        return result;
      }),
    )
  ).filter((user) => user !== null);

  linkLog("CollectInfo", `Resolved ${userLinksAndInfos.length} player records`);

  linkLog("Guild", "Fetching guild info");
  const guild = await client.guilds.fetch(config.guildId);
  if (!guild) return console.error("Guild not found");

  await guild.members.fetch(); // Ensure all members are cached

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

  linkLog("Assign", "Assigning guild roles");
  for (const [key, { id, predicate }] of Object.entries(roles)) {
    linkLog(key, `Preparing role ${id}`);
    const role = await guild.roles.fetch(id);
    if (!role) {
      linkLog(key, "Role not found, skipping");
      continue;
    }

    const allowedUserIds = userLinksAndInfos
      .filter((user) => predicate(user))
      .map((user) => user.discordId)
      .filter((id) => !bypassMembers.has(id));
    const membersInRole = role.members
      .map((member) => member.id)
      .filter((id) => !bypassMembers.has(id));
    const membersToAdd = allowedUserIds.filter(
      (user) => !membersInRole.includes(user),
    );
    const membersToRemove = membersInRole.filter(
      (member) => !allowedUserIds.includes(member),
    );

    linkLog(
      key,
      `Allowed=${allowedUserIds.length}, current=${membersInRole.length}, add=${membersToAdd.length}, remove=${membersToRemove.length}`,
    );

    let addTriedCount = 0;
    let addMissingCount = 0;
    let addSuccessCount = 0;
    for (const memberId of membersToAdd) {
      addTriedCount++;
      const member = guild.members.cache.get(memberId);
      if (!member) {
        addMissingCount++;
        continue;
      }
      addSuccessCount++;
      await member.roles.add(role);
    }

    linkLog(
      key,
      `Add summary: tried=${addTriedCount}, succeeded=${addSuccessCount}, missing=${addMissingCount}`,
    );

    let removeTriedCount = 0;
    let removeMissingCount = 0;
    let removeSuccessCount = 0;
    for (const memberId of membersToRemove) {
      removeTriedCount++;
      const member = guild.members.cache.get(memberId);
      if (!member) {
        removeMissingCount++;
        continue;
      }
      removeSuccessCount++;
      await member.roles.remove(role);
    }

    linkLog(
      key,
      `Remove summary: tried=${removeTriedCount}, succeeded=${removeSuccessCount}, missing=${removeMissingCount}`,
    );
  }

  linkLog("Assign", "Linked role sync complete");
};
