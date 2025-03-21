import {Client, Snowflake} from 'discord.js';
import {collectUserInfo, getAllLinkedUsers, UserInfo, UserLinkAndInfo} from "../utils/helpers";
import config from "../config/config";

const roles: Record<string, {
  id: Snowflake,
  predicate: (check: UserInfo) => boolean
}> = {
  'linked': {
    id: '1325507259307921428',
    predicate: () => true
  },
  'prime': {
    id: '1268337190144835718',
    predicate: (info) => info.topRank === 'prime'
  },
  'primeultra': {
    id: '1325147393372586054',
    predicate: (info) => info.topRank === 'primeultra'
  },
  'elite': {
    id: '1268337279898878013',
    predicate: (info) => info.topRank === 'elite'
  },
  'eliteultra': {
    id: '1325147417322192927',
    predicate: (info) => info.topRank === 'eliteultra'
  },
  'apex': {
    id: '1268345919003430942',
    predicate: (info) => info.topRank === 'apex'
  },
  'apexultra': {
    id: '1349026308390391839',
    predicate: (info) => info.topRank === 'apexultra'
  },
  '2022': {
    id: "1349065372313321514",
    predicate: (info) => info.firstJoinYear <= 2022
  },
  '2023': {
    id: "1349065403477004480",
    predicate: (info) => info.firstJoinYear === 2023
  },
  '2024': {
    id: "1349065422065893516",
    predicate: (info) => info.firstJoinYear === 2024
  },
  '2025': {
    id: "1349065443650043955",
    predicate: (info) => info.firstJoinYear === 2025
  },
};

export const sync = async (client: Client) => {
  console.log('Getting linked users...');
  const linkedUsers = await getAllLinkedUsers();

  console.log('Collecting user info...');
  const userLinksAndInfos: UserLinkAndInfo[] = (await Promise.all(linkedUsers.map(async linkedUser => {
    const userInfo = await collectUserInfo(linkedUser.minecraftUuid);
    if (userInfo === null) return null;

    const result: UserLinkAndInfo = {
      ...linkedUser,
      ...userInfo
    }
    return result;
  }))).filter(user => user !== null);

  console.log("Getting guild info...");
  const guild = client.guilds.cache.get(config.guildId);
  if (!guild) return console.error('Guild not found');

  await guild.members.fetch(); // Ensure all members are cached

  console.log("Assigning guild roles...");
  for (const [key, {id, predicate}] of Object.entries(roles)) {
    const role = guild.roles.cache.get(id);
    if (!role) return console.log('Role not found');

    const allowedUsers = userLinksAndInfos.filter(user => predicate(user));
    const membersInRole = role.members.map(member => member.id);
    const membersToAdd = allowedUsers.filter(user => !membersInRole.includes(user.discordId));
    const membersToRemove = membersInRole.filter(member => !allowedUsers.map(user => user.discordId).includes(member));

    for (const userLink of membersToAdd) {
      console.log(`Adding ${key} to ${userLink.discordId}`);
      await guild.members.cache.get(userLink.discordId)?.roles.add(role);
    }

    for (const memberId of membersToRemove) {
      console.log(`Removing ${key} from ${memberId}`);
      await guild.members.cache.get(memberId)?.roles.remove(role);
    }
  }

  console.log("Sync complete");
};
