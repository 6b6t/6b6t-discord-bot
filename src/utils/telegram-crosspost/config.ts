export type TelegramCrosspostRoute = {
  id: string;
  discordChannelId: string;
  telegramChatId: string;
  telegramThreadId: number | null;
  includeAuthor: boolean;
};

export type TelegramCrosspostConfig = {
  enabled: boolean;
  token: string | null;
  routes: TelegramCrosspostRoute[];
  syncEdits: boolean;
  syncDeletes: boolean;
  backfillOnFirstRun: boolean;
  retryAttempts: number;
};

type CrosspostRouteInput = {
  id?: unknown;
  discordChannelId?: unknown;
  telegramChatId?: unknown;
  telegramThreadId?: unknown;
  includeAuthor?: unknown;
};

function parseBoolean(value: string | undefined, fallback: boolean) {
  if (value === undefined) return fallback;
  return value.trim().toLowerCase() === "true";
}

function parsePositiveInteger(
  value: string | undefined,
  fallback: number,
  maximum: number,
) {
  if (!value) return fallback;
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed < 1) return fallback;
  return Math.min(parsed, maximum);
}

function requireString(value: unknown, field: string, index: number) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(
      `TELEGRAM_CROSSPOST_ROUTES[${index}].${field} must be a non-empty string`,
    );
  }
  return value.trim();
}

function parseThreadId(value: unknown, index: number): number | null {
  if (value === undefined || value === null || value === "") return null;
  const parsed =
    typeof value === "number" ? value : Number.parseInt(String(value), 10);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(
      `TELEGRAM_CROSSPOST_ROUTES[${index}].telegramThreadId must be a positive integer`,
    );
  }
  return parsed;
}

export function parseTelegramCrosspostRoutes(
  rawValue: string | undefined,
): TelegramCrosspostRoute[] {
  if (!rawValue?.trim()) return [];

  let parsed: unknown;
  try {
    parsed = JSON.parse(rawValue);
  } catch {
    throw new Error("TELEGRAM_CROSSPOST_ROUTES must be valid JSON");
  }

  if (!Array.isArray(parsed)) {
    throw new Error("TELEGRAM_CROSSPOST_ROUTES must be a JSON array");
  }

  const routeIds = new Set<string>();
  return parsed.map((value: CrosspostRouteInput, index) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error(`TELEGRAM_CROSSPOST_ROUTES[${index}] must be an object`);
    }

    const id = requireString(value.id, "id", index);
    if (!/^[a-zA-Z0-9_-]{1,64}$/.test(id)) {
      throw new Error(
        `TELEGRAM_CROSSPOST_ROUTES[${index}].id may only contain letters, numbers, _ and -`,
      );
    }
    if (routeIds.has(id)) {
      throw new Error(`Duplicate Telegram crosspost route id: ${id}`);
    }
    routeIds.add(id);

    const discordChannelId = requireString(
      value.discordChannelId,
      "discordChannelId",
      index,
    );
    if (!/^\d{5,32}$/.test(discordChannelId)) {
      throw new Error(
        `TELEGRAM_CROSSPOST_ROUTES[${index}].discordChannelId is invalid`,
      );
    }

    const telegramChatId = requireString(
      value.telegramChatId,
      "telegramChatId",
      index,
    );
    if (
      !/^-?\d{5,32}$/.test(telegramChatId) &&
      !/^@[A-Za-z0-9_]{5,32}$/.test(telegramChatId)
    ) {
      throw new Error(
        `TELEGRAM_CROSSPOST_ROUTES[${index}].telegramChatId is invalid`,
      );
    }

    return {
      id,
      discordChannelId,
      telegramChatId,
      telegramThreadId: parseThreadId(value.telegramThreadId, index),
      includeAuthor: value.includeAuthor === true,
    };
  });
}

export function getTelegramCrosspostConfig(
  environment: NodeJS.ProcessEnv = process.env,
): TelegramCrosspostConfig {
  const routes = parseTelegramCrosspostRoutes(
    environment.TELEGRAM_CROSSPOST_ROUTES,
  );
  const token = environment.TELEGRAM_CROSSPOST_BOT_TOKEN?.trim() || null;

  if (routes.length > 0 && !token) {
    throw new Error(
      "TELEGRAM_CROSSPOST_BOT_TOKEN is required when TELEGRAM_CROSSPOST_ROUTES is configured",
    );
  }

  return {
    enabled: routes.length > 0 && token !== null,
    token,
    routes,
    syncEdits: parseBoolean(environment.TELEGRAM_CROSSPOST_SYNC_EDITS, true),
    syncDeletes: parseBoolean(
      environment.TELEGRAM_CROSSPOST_SYNC_DELETES,
      false,
    ),
    backfillOnFirstRun: parseBoolean(
      environment.TELEGRAM_CROSSPOST_BACKFILL_ON_FIRST_RUN,
      false,
    ),
    retryAttempts: parsePositiveInteger(
      environment.TELEGRAM_CROSSPOST_RETRY_ATTEMPTS,
      6,
      12,
    ),
  };
}
