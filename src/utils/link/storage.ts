import type { RowDataPacket } from "mysql2/promise";
import { getLinkPool } from "./mariadb-client";
import { ensureLinkSchema } from "./schema-manager";
import type { DiscordRoleMetadata, DiscordTokens } from "./types";

type MetadataRow = RowDataPacket & { metadata: string };
type TokenRow = RowDataPacket & {
  user_id: string;
  access_token: string;
  refresh_token: string;
  expires_in: number | string;
  expires_at: number | string;
};
type MappingRow = RowDataPacket & { uuid: string; discord_id: string };

type MetadataObject = DiscordRoleMetadata;

export function normalizeMetadata(value: unknown): MetadataObject | null {
  if (!value || typeof value !== "object") {
    return null;
  }

  const metadataEntries = Object.entries(
    value as Record<string, unknown>,
  ).filter((entry): entry is [string, string | number] => {
    const entryValue = entry[1];
    return typeof entryValue === "string" || typeof entryValue === "number";
  });

  return Object.fromEntries(metadataEntries) as MetadataObject;
}

export async function upsertDiscordTokens(
  userId: string,
  tokens: DiscordTokens,
) {
  await ensureLinkSchema();
  const pool = getLinkPool();
  await pool.execute(
    `INSERT INTO discord_tokens (user_id, access_token, refresh_token, expires_in, expires_at)
     VALUES (?, ?, ?, ?, ?)
     ON DUPLICATE KEY UPDATE
       access_token = VALUES(access_token),
       refresh_token = VALUES(refresh_token),
       expires_in = VALUES(expires_in),
       expires_at = VALUES(expires_at)`,
    [
      userId,
      tokens.access_token,
      tokens.refresh_token,
      tokens.expires_in,
      tokens.expires_at,
    ],
  );
}

export async function getDiscordTokensFromDb(
  userId: string,
): Promise<DiscordTokens | null> {
  await ensureLinkSchema();
  const pool = getLinkPool();
  const [rows] = await pool.query<TokenRow[]>(
    `SELECT user_id, access_token, refresh_token, expires_in, expires_at
     FROM discord_tokens
     WHERE user_id = ?
     LIMIT 1`,
    [userId],
  );

  if (!rows.length) {
    return null;
  }

  const row = rows[0];
  return {
    access_token: row.access_token,
    refresh_token: row.refresh_token,
    expires_in: Number(row.expires_in),
    expires_at: Number(row.expires_at),
  };
}

export async function upsertLastMetadata(
  userId: string,
  metadata: MetadataObject,
) {
  await ensureLinkSchema();
  const pool = getLinkPool();
  await pool.execute(
    `INSERT INTO discord_role_metadata (user_id, metadata)
     VALUES (?, ?)
     ON DUPLICATE KEY UPDATE metadata = VALUES(metadata)`,
    [userId, JSON.stringify(metadata)],
  );
}

export async function getLastMetadataFromDb(
  userId: string,
): Promise<MetadataObject | null> {
  await ensureLinkSchema();
  const pool = getLinkPool();
  const [rows] = await pool.query<MetadataRow[]>(
    `SELECT metadata
     FROM discord_role_metadata
     WHERE user_id = ?
     LIMIT 1`,
    [userId],
  );

  if (!rows.length) {
    return null;
  }

  try {
    const parsed = JSON.parse(rows[0].metadata) as unknown;
    return normalizeMetadata(parsed);
  } catch (error) {
    console.error("Failed to parse stored metadata", error);
    return null;
  }
}

export async function upsertUuidToDiscordMapping(
  uuid: string,
  discordId: string,
) {
  await ensureLinkSchema();
  const pool = getLinkPool();
  await pool.execute(
    `INSERT INTO uuid_to_discord (uuid, discord_id)
     VALUES (?, ?)
     ON DUPLICATE KEY UPDATE discord_id = VALUES(discord_id)`,
    [uuid, discordId],
  );
}

export async function getDiscordIdForUuid(
  uuid: string,
): Promise<string | null> {
  await ensureLinkSchema();
  const pool = getLinkPool();
  const [rows] = await pool.query<MappingRow[]>(
    `SELECT uuid, discord_id
     FROM uuid_to_discord
     WHERE uuid = ?
     LIMIT 1`,
    [uuid],
  );

  if (!rows.length) {
    return null;
  }

  return rows[0].discord_id;
}

export async function removeMappingsForDiscordId(discordId: string) {
  await ensureLinkSchema();
  const pool = getLinkPool();
  await pool.execute(`DELETE FROM uuid_to_discord WHERE discord_id = ?`, [
    discordId,
  ]);
}

export async function getAllMappedUuids(): Promise<string[]> {
  await ensureLinkSchema();
  const pool = getLinkPool();
  const [rows] = await pool.query<MappingRow[]>(
    `SELECT uuid
     FROM uuid_to_discord`,
  );
  return rows.map((row) => row.uuid);
}

export async function getAllUuidDiscordMappings() {
  await ensureLinkSchema();
  const pool = getLinkPool();
  const [rows] = await pool.query<MappingRow[]>(
    `SELECT uuid, discord_id
     FROM uuid_to_discord`,
  );
  return rows.map((row) => ({ uuid: row.uuid, discordId: row.discord_id }));
}
