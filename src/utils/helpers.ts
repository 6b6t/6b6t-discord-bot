import {RowDataPacket} from "mysql2";
import {getStatsPool} from "./mysql-client";
import {getRedisClient} from "./redis";
import { Client, TextChannel } from "discord.js";
import config from "../config/config";

export type UserInfo = {
  topRank: string;
  firstJoinYear: number;
}

export type UserLink = {
  discordId: string;
  minecraftUuid: string;
}

export type UserLinkAndInfo = UserLink & UserInfo;

export async function getAllLinkedUsers(): Promise<UserLink[]> {
  let result: UserLink[] = [];
  const redisClient = await getRedisClient();
  for (const key of await redisClient.keys('uuid_to_discord_id:*')) {
    const valueDiscordId = await redisClient.get(key);
    const keyUuid = key.split(':')[1];
    if (valueDiscordId !== null) {
      result.push({
        discordId: valueDiscordId,
        minecraftUuid: keyUuid,
      });
    }
  }

  return result;
}

export async function collectUserInfo(uuid: string): Promise<UserInfo | null> {
  const playerInfo = await findPlayerInfoByUuid(uuid);
  if (playerInfo.length === 0) {
    return null;
  }

  const topRank = await getTopRank(playerInfo[0].name);
  if (topRank === null) {
    return null;
  }

  const firstJoinYear = new Date(playerInfo[0].first_join).getFullYear();
  return {
    topRank: topRank,
    firstJoinYear,
  };
}

export async function findPlayerInfoByUuid(
    uuid: string,
): Promise<RowDataPacket[]> {
  console.time(`findPlayerInfo-${uuid}`);
  try {
    const [rows] = await getStatsPool().execute<RowDataPacket[]>(
        `
            SELECT uuid, name, texture_hash, first_join
            FROM player_info
            WHERE uuid = ?
        `,
        [uuid],
    );

    return rows;
  } catch (e) {
    console.error(e);
    throw new Error('Failed to find player info');
  } finally {
    console.timeEnd(`findPlayerInfo-${uuid}`);
  }
}

export async function getTopRank(
    username: string,
): Promise<string | null> {
  const response = await (
      await fetch(`${process.env.HTTP_COMMAND_SERVICE_BASE_URL}/get-ranks`, {
        method: 'POST',
        body: JSON.stringify({username}),
        headers: {
          Accept: 'application/json',
          'Content-Type': 'application/json',
          Authorization: `${process.env.HTTP_COMMAND_SERVICE_ACCESS_TOKEN}`,
        },
      })
  ).json();
  if (response.success !== true) {
    throw new Error(response.error);
  } else if (response['user-not-found'] === true) {
    return null;
  }

  let topRank = 'default';
  const ranks: string[] = response.ranks;
  if (ranks.includes('apexultra')) {
    topRank = 'apexultra';
  } else if (ranks.includes('apex')) {
    topRank = 'apex';
  } else if (ranks.includes('eliteultra')) {
    topRank = 'eliteultra';
  } else if (ranks.includes('elite')) {
    topRank = 'elite';
  } else if (ranks.includes('primeultra')) {
    topRank = 'primeultra';
  } else if (ranks.includes('prime')) {
    topRank = 'prime';
  }

  return topRank;
}

export async function deleteAdvertisingMessage(client: Client, channel: TextChannel) {
  const messages = await channel.messages.fetch({ limit: 100 });
  const botMessage = messages.find((msg) => msg.author.id === client.user?.id);

  if (botMessage) {
    try {
      await botMessage.delete();
    } catch (error) {
      console.error("Failed to delete advertising message: ", error);
    }
  }
}
