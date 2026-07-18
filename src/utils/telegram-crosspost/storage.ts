import type { RowDataPacket } from "mysql2/promise";
import { getLinkPool } from "../link/mariadb-client";
import { ensureLinkSchema } from "../link/schema-manager";
import type { TelegramMessageReference } from "./telegram-client";

export type CrosspostStatus = "pending" | "sent" | "failed" | "deleted";

export type CrosspostRecord = {
  routeId: string;
  discordMessageId: string;
  discordChannelId: string;
  telegramChatId: string;
  contentHash: string;
  status: CrosspostStatus;
  telegramMessages: TelegramMessageReference[];
  attemptCount: number;
  lastError: string | null;
};

type CrosspostRow = RowDataPacket & {
  route_id: string;
  discord_message_id: string;
  discord_channel_id: string;
  telegram_chat_id: string;
  content_hash: string;
  status: CrosspostStatus;
  telegram_messages: string | null;
  attempt_count: number | string;
  last_error: string | null;
};

type RouteStateRow = RowDataPacket & {
  last_discord_message_id: string | null;
};

let crosspostSchemaInitialized = false;

export async function ensureCrosspostStorage() {
  if (crosspostSchemaInitialized) return;

  await ensureLinkSchema();
  const pool = getLinkPool();
  await pool.query(`CREATE TABLE IF NOT EXISTS telegram_crossposts (
    route_id VARCHAR(64) NOT NULL,
    discord_message_id VARCHAR(64) NOT NULL,
    discord_channel_id VARCHAR(64) NOT NULL,
    telegram_chat_id VARCHAR(64) NOT NULL,
    content_hash CHAR(64) NOT NULL,
    status VARCHAR(16) NOT NULL,
    telegram_messages LONGTEXT NULL,
    attempt_count INT NOT NULL DEFAULT 0,
    last_error TEXT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (route_id, discord_message_id),
    INDEX idx_telegram_crossposts_status (status)
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`);
  await pool.query(`CREATE TABLE IF NOT EXISTS telegram_crosspost_routes (
    route_id VARCHAR(64) NOT NULL PRIMARY KEY,
    last_discord_message_id VARCHAR(64) NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
  ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci`);

  crosspostSchemaInitialized = true;
}

function parseTelegramMessages(value: string | null) {
  if (!value) return [];
  try {
    const parsed = JSON.parse(value) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (item): item is TelegramMessageReference =>
        Boolean(item) &&
        typeof item === "object" &&
        Number.isInteger((item as TelegramMessageReference).messageId) &&
        ["text", "photo", "video", "document"].includes(
          (item as TelegramMessageReference).kind,
        ),
    );
  } catch {
    return [];
  }
}

function mapRow(row: CrosspostRow): CrosspostRecord {
  return {
    routeId: row.route_id,
    discordMessageId: row.discord_message_id,
    discordChannelId: row.discord_channel_id,
    telegramChatId: row.telegram_chat_id,
    contentHash: row.content_hash,
    status: row.status,
    telegramMessages: parseTelegramMessages(row.telegram_messages),
    attemptCount: Number(row.attempt_count),
    lastError: row.last_error,
  };
}

export async function getCrosspostRecord(
  routeId: string,
  discordMessageId: string,
) {
  await ensureCrosspostStorage();
  const [rows] = await getLinkPool().query<CrosspostRow[]>(
    `SELECT route_id, discord_message_id, discord_channel_id,
            telegram_chat_id, content_hash, status, telegram_messages,
            attempt_count, last_error
     FROM telegram_crossposts
     WHERE route_id = ? AND discord_message_id = ?
     LIMIT 1`,
    [routeId, discordMessageId],
  );
  return rows[0] ? mapRow(rows[0]) : null;
}

export async function beginCrosspost(input: {
  routeId: string;
  discordMessageId: string;
  discordChannelId: string;
  telegramChatId: string;
  contentHash: string;
}) {
  await ensureCrosspostStorage();
  await getLinkPool().execute(
    `INSERT INTO telegram_crossposts
       (route_id, discord_message_id, discord_channel_id, telegram_chat_id,
        content_hash, status, attempt_count)
     VALUES (?, ?, ?, ?, ?, 'pending', 1)
     ON DUPLICATE KEY UPDATE
       discord_channel_id = VALUES(discord_channel_id),
       telegram_chat_id = VALUES(telegram_chat_id),
       content_hash = VALUES(content_hash),
       status = 'pending',
       attempt_count = attempt_count + 1,
       last_error = NULL`,
    [
      input.routeId,
      input.discordMessageId,
      input.discordChannelId,
      input.telegramChatId,
      input.contentHash,
    ],
  );
}

export async function markCrosspostSent(
  routeId: string,
  discordMessageId: string,
  telegramMessages: TelegramMessageReference[],
) {
  await ensureCrosspostStorage();
  await getLinkPool().execute(
    `UPDATE telegram_crossposts
     SET status = 'sent', telegram_messages = ?, last_error = NULL
     WHERE route_id = ? AND discord_message_id = ?`,
    [JSON.stringify(telegramMessages), routeId, discordMessageId],
  );
}

export async function markCrosspostFailed(
  routeId: string,
  discordMessageId: string,
  error: string,
  partialMessages: TelegramMessageReference[],
) {
  await ensureCrosspostStorage();
  await getLinkPool().execute(
    `UPDATE telegram_crossposts
     SET status = 'failed', telegram_messages = ?, last_error = ?
     WHERE route_id = ? AND discord_message_id = ?`,
    [
      JSON.stringify(partialMessages),
      error.slice(0, 2_000),
      routeId,
      discordMessageId,
    ],
  );
}

export async function markCrosspostDeleted(
  routeId: string,
  discordMessageId: string,
) {
  await ensureCrosspostStorage();
  await getLinkPool().execute(
    `UPDATE telegram_crossposts
     SET status = 'deleted', telegram_messages = '[]', last_error = NULL
     WHERE route_id = ? AND discord_message_id = ?`,
    [routeId, discordMessageId],
  );
}

export async function getRecoverableCrossposts(limit = 200) {
  await ensureCrosspostStorage();
  const normalizedLimit = Math.max(1, Math.min(Math.floor(limit), 1_000));
  const [rows] = await getLinkPool().query<CrosspostRow[]>(
    `SELECT route_id, discord_message_id, discord_channel_id,
            telegram_chat_id, content_hash, status, telegram_messages,
            attempt_count, last_error
     FROM telegram_crossposts
     WHERE status IN ('pending', 'failed')
     ORDER BY updated_at ASC
     LIMIT ${normalizedLimit}`,
  );
  return rows.map(mapRow);
}

export async function getRouteCheckpoint(routeId: string) {
  await ensureCrosspostStorage();
  const [rows] = await getLinkPool().query<RouteStateRow[]>(
    `SELECT last_discord_message_id
     FROM telegram_crosspost_routes
     WHERE route_id = ?
     LIMIT 1`,
    [routeId],
  );
  return rows[0]?.last_discord_message_id ?? null;
}

export async function setRouteCheckpoint(
  routeId: string,
  discordMessageId: string,
) {
  await ensureCrosspostStorage();
  await getLinkPool().execute(
    `INSERT INTO telegram_crosspost_routes (route_id, last_discord_message_id)
     VALUES (?, ?)
     ON DUPLICATE KEY UPDATE
       last_discord_message_id = VALUES(last_discord_message_id)`,
    [routeId, discordMessageId],
  );
}
