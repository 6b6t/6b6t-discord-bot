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

export type HytalePlayerSummary = {
  Name: string;
  UUID: string;
  World: string;
};

export type HytaleMetrics = {
  tps: number | null;
  entities: number | null;
  chunks: number | null;
};

export type HytalePlayerCountData = {
  playerCount: number;
  maxPlayers: number;
  players: HytalePlayerSummary[];
  metrics?: HytaleMetrics;
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
          Authorization: `Bearer ${process.env.HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN}`,
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

function sumMetrics(text: string, metricName: string): number | null {
  const regex = new RegExp(`^${metricName}(?:\\{.*?\\})? ([\\d.]+)`, "gm");
  let sum = 0;
  let found = false;
  for (const match of text.matchAll(regex)) {
    sum += parseFloat(match[1]);
    found = true;
  }
  return found ? sum : null;
}

function getAverageMetric(text: string, metricName: string): number | null {
  const regex = new RegExp(`^${metricName}(?:\\{.*?\\})? ([\\d.]+)`, "gm");
  let sum = 0;
  let count = 0;
  for (const match of text.matchAll(regex)) {
    sum += parseFloat(match[1]);
    count++;
  }
  return count > 0 ? sum / count : null;
}

export async function getHytalePlayerCountData(): Promise<HytalePlayerCountData | null> {
  const endpointUrl = process.env.HYTALE_QUERY_ENDPOINT_URL;
  const username = process.env.HYTALE_QUERY_USERNAME;
  const password = process.env.HYTALE_QUERY_PASSWORD;

  if (!endpointUrl || !username || !password) {
    console.error(
      "HYTALE_QUERY_ENDPOINT_URL, HYTALE_QUERY_USERNAME, and HYTALE_QUERY_PASSWORD must be set",
    );
    return null;
  }

  const metricsUrl = new URL(
    "/ApexHosting/PrometheusExporter/metrics",
    endpointUrl,
  ).toString();

  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), 5000);

  try {
    const authHeader = `Basic ${Buffer.from(`${username}:${password}`).toString("base64")}`;

    const [response, metricsResponse] = await Promise.all([
      fetch(endpointUrl, {
        headers: {
          Accept: "application/json",
          Authorization: authHeader,
        },
        signal: controller.signal,
      }),
      fetch(metricsUrl, {
        headers: {
          Authorization: authHeader,
        },
        signal: controller.signal,
      }).catch((e) => {
        console.error("Error fetching Hytale metrics:", e);
        return null;
      }),
    ]);

    let metrics: HytaleMetrics | undefined;
    if (metricsResponse?.ok) {
      const text = await metricsResponse.text();
      metrics = {
        tps: getAverageMetric(text, "hytale_world_tps_avg"),
        entities: sumMetrics(text, "hytale_entities_active"),
        chunks: sumMetrics(text, "hytale_chunks_active"),
      };
    } else if (metricsResponse && !metricsResponse.ok) {
      console.error(
        "Error fetching Hytale metrics:",
        metricsResponse.status,
        metricsResponse.statusText,
      );
    }

    if (!response.ok) {
      console.error(
        "Error fetching Hytale player count:",
        response.status,
        response.statusText,
      );
      return null;
    }

    const payload = (await response.json()) as {
      Server?: { MaxPlayers?: unknown };
      Universe?: { CurrentPlayers?: unknown };
      Players?: unknown;
    };

    if (
      typeof payload.Server?.MaxPlayers !== "number" ||
      typeof payload.Universe?.CurrentPlayers !== "number" ||
      !Array.isArray(payload.Players)
    ) {
      console.error("Invalid Hytale player count response payload");
      return null;
    }

    return {
      playerCount: payload.Universe.CurrentPlayers,
      maxPlayers: payload.Server.MaxPlayers,
      players: payload.Players.filter(
        (player): player is HytalePlayerSummary =>
          typeof player === "object" &&
          player !== null &&
          typeof player.Name === "string" &&
          typeof player.UUID === "string" &&
          typeof player.World === "string",
      ),
      metrics,
    };
  } catch (error) {
    console.error("Error fetching Hytale player count:", error);
    return null;
  } finally {
    clearTimeout(timeoutId);
  }
}
