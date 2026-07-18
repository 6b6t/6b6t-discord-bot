import assert from "node:assert/strict";
import test from "node:test";
import {
  getTelegramCrosspostConfig,
  parseTelegramCrosspostRoutes,
} from "./config";

test("parses multiple Discord to Telegram routes", () => {
  const routes = parseTelegramCrosspostRoutes(
    JSON.stringify([
      {
        id: "announcements",
        discordChannelId: "1234567890",
        telegramChatId: "-1001234567890",
        label: "Announcements",
      },
      {
        id: "changelog",
        discordChannelId: "2234567890",
        telegramChatId: "@example_channel",
        telegramThreadId: 42,
        includeAuthor: true,
      },
    ]),
  );

  assert.equal(routes.length, 2);
  assert.deepEqual(routes[1], {
    id: "changelog",
    discordChannelId: "2234567890",
    telegramChatId: "@example_channel",
    telegramThreadId: 42,
    includeAuthor: true,
  });
});

test("rejects duplicate route ids", () => {
  assert.throws(
    () =>
      parseTelegramCrosspostRoutes(
        JSON.stringify([
          {
            id: "updates",
            discordChannelId: "1234567890",
            telegramChatId: "-1001234567890",
          },
          {
            id: "updates",
            discordChannelId: "2234567890",
            telegramChatId: "-1002234567890",
          },
        ]),
      ),
    /Duplicate Telegram crosspost route id/,
  );
});

test("requires an explicit crosspost token when routes are enabled", () => {
  assert.throws(
    () =>
      getTelegramCrosspostConfig({
        TELEGRAM_CROSSPOST_ROUTES: JSON.stringify([
          {
            id: "updates",
            discordChannelId: "1234567890",
            telegramChatId: "-1001234567890",
          },
        ]),
      }),
    /TELEGRAM_CROSSPOST_BOT_TOKEN is required/,
  );
});

test("is disabled without routes and applies safe defaults", () => {
  const config = getTelegramCrosspostConfig({});
  assert.equal(config.enabled, false);
  assert.equal(config.syncEdits, true);
  assert.equal(config.syncDeletes, false);
  assert.equal(config.backfillOnFirstRun, false);
  assert.equal(config.retryAttempts, 6);
});

test("enables Telegram deletion only when explicitly configured", () => {
  const config = getTelegramCrosspostConfig({
    TELEGRAM_CROSSPOST_BOT_TOKEN: "token",
    TELEGRAM_CROSSPOST_ROUTES: JSON.stringify([
      {
        id: "updates",
        discordChannelId: "1234567890",
        telegramChatId: "-1001234567890",
      },
    ]),
    TELEGRAM_CROSSPOST_SYNC_DELETES: "true",
  });

  assert.equal(config.syncDeletes, true);
});
