import type { Client, TextChannel } from "discord.js";
import type { RowDataPacket } from "mysql2";
import { getAllUuidDiscordMappings } from "./link/storage";
import { getStatsPool } from "./mysql-client";

const SERVER_API = "https://www.6b6t.org/api";

export type UserInfo = {
  topRank: string;
  firstJoinYear: number;
};

export type UserLink = {
  discordId: string;
  minecraftUuid: string;
};

export type UserLinkAndInfo = UserLink & UserInfo;

export type PlayerSummary = {
  uuid: string;
  username: string;
};

export type UptimeData = {
  serverStartUnix?: number;
  currentUptimeHours?: number;
};

export type ServerData = {
  playerCount: number;
  players: PlayerSummary[];
  version?: string;
  uptime?: UptimeData;
};

export function formatDuration(totalSeconds: number): string {
  const days = Math.floor(totalSeconds / 86400);
  const hours = Math.floor((totalSeconds % 86400) / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const parts: string[] = [];
  if (days > 0) parts.push(`${days}d`);
  if (hours > 0) parts.push(`${hours}h`);
  if (minutes > 0) parts.push(`${minutes}m`);
  if (seconds > 0) parts.push(`${seconds}s`);
  return parts.join(" ") || "0s";
}

export async function getAllLinkedUsers(): Promise<UserLink[]> {
  const mappings = await getAllUuidDiscordMappings();
  return mappings.map((mapping) => ({
    discordId: mapping.discordId,
    minecraftUuid: mapping.uuid,
  }));
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
    throw new Error("Failed to find player info");
  }
}

export async function getTopRank(username: string): Promise<string | null> {
  const response = await (
    await fetch(
      `${process.env.HTTP_SLAVE1_COMMAND_SERVICE_BASE_URL}/get-ranks`,
      {
        method: "POST",
        body: JSON.stringify({ username }),
        headers: {
          Accept: "application/json",
          "Content-Type": "application/json",
          Authorization: `${process.env.HTTP_SLAVE1_COMMAND_SERVICE_ACCESS_TOKEN}`,
        },
      },
    )
  ).json();
  if (response.success !== true) {
    throw new Error(response.error);
  } else if (response["user-not-found"] === true) {
    return null;
  }

  let topRank = "default";
  const ranks: string[] = response.ranks;
  if (ranks.includes("legend")) {
    topRank = "legend";
  } else if (ranks.includes("apex")) {
    topRank = "apex";
  } else if (ranks.includes("eliteultra")) {
    topRank = "eliteultra";
  } else if (ranks.includes("elite")) {
    topRank = "elite";
  } else if (ranks.includes("primeultra")) {
    topRank = "primeultra";
  } else if (ranks.includes("prime")) {
    topRank = "prime";
  }

  return topRank;
}

export async function getPlayerByDiscordId(
  discordId: string,
): Promise<{ name: string; topRank: string; firstJoinYear: number } | null> {
  const linkedUsers = await getAllLinkedUsers();
  const userLink = linkedUsers.find((u) => u.discordId === discordId);
  if (!userLink) return null;

  const playerInfoRows = await findPlayerInfoByUuid(userLink.minecraftUuid);
  if (playerInfoRows.length === 0) return null;

  const playerInfo = playerInfoRows[0];
  const topRank = await getTopRank(playerInfo.name);
  if (!topRank) return null;

  return {
    name: playerInfo.name,
    topRank,
    firstJoinYear: new Date(playerInfo.first_join).getFullYear(),
  };
}

export async function deleteLatestMessage(
  client: Client,
  channel: TextChannel,
  limit: number = 100,
) {
  const messages = await channel.messages.fetch({ limit: limit });
  const botMessage = messages.find((msg) => msg.author.id === client.user?.id);

  if (botMessage) {
    try {
      await botMessage.delete();
    } catch (error) {
      console.error(
        `Failed to delete latest message in ${channel.id}: `,
        error,
      );
    }
  }
}

export async function botHasRecentMessages(
  channel: TextChannel,
  client: Client,
  limit: number = 100,
) {
  const messages = await channel.messages.fetch({ limit });
  return messages.filter((msg) => msg.author.id === client.user?.id).size;
}

async function fetchPlayersFromCommandService(): Promise<ServerData | null> {
  try {
    const response = await fetch(
      `${process.env.HTTP_PROXY_COMMAND_SERVICE_BASE_URL}/players`,
      {
        headers: {
          Accept: "application/json",
          Authorization: `${process.env.HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN}`,
        },
      },
    );

    if (response.status === 403) {
      console.error(
        "HTTP command service rejected the players request (missing can-get-players permission)",
      );
      return null;
    }

    if (!response.ok) {
      console.error(
        "HTTP command service player request failed:",
        await response.text(),
      );
      return null;
    }

    const payload = (await response.json()) as {
      success: boolean;
      "player-count": number;
      players?: PlayerSummary[];
    };

    if (!payload.success) {
      console.error(
        "HTTP command service returned an unsuccessful player response",
      );
      return null;
    }

    return {
      playerCount: payload["player-count"],
      players: payload.players ?? [],
    };
  } catch (error) {
    console.error("Error fetching players from HTTP command service:", error);
    return null;
  }
}

export async function getServerData(): Promise<ServerData | null> {
  const versionUrl = `${SERVER_API}/version`;
  const uptimeUrl = `${SERVER_API}/uptime`;

  try {
    const [players, version, uptimeRes] = await Promise.all([
      fetchPlayersFromCommandService(),
      fetch(versionUrl)
        .then((res) => (res.ok ? res.json() : null))
        .catch((error) => {
          console.error("Error fetching version data:", error);
          return null;
        }),
      fetch(uptimeUrl)
        .then((res) => (res.ok ? res.json() : null))
        .catch((error) => {
          console.error("Error fetching uptime data:", error);
          return null;
        }),
    ]);

    if (!players) return null;

    const uptime: UptimeData | undefined = uptimeRes?.statistics
      ? {
          serverStartUnix: uptimeRes.statistics.serverStartUnix,
          currentUptimeHours: uptimeRes.statistics.currentUptimeHours,
        }
      : undefined;

    return { ...players, version: version?.version, uptime };
  } catch (error) {
    console.error("Error fetching server data:", error);
    return null;
  }
}
