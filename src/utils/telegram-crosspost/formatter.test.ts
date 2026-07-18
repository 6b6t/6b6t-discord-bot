import assert from "node:assert/strict";
import test from "node:test";
import type { TelegramCrosspostRoute } from "./config";
import {
  buildTelegramCrosspostPayload,
  normalizeDiscordMarkdown,
  splitTelegramText,
} from "./formatter";

const route: TelegramCrosspostRoute = {
  id: "updates",
  discordChannelId: "1234567890",
  telegramChatId: "-1001234567890",
  telegramThreadId: null,
  includeAuthor: true,
};

test("normalizes Discord-only markdown and mentions", () => {
  assert.equal(
    normalizeDiscordMarkdown(
      "# **Update** <@123456> <@&223456> <#323456> <:party:423456> [Shop](https://www.6b6t.org/shop)",
    ),
    "Update Shop (https://www.6b6t.org/shop)",
  );
});

test("removes mentions and Discord-only formatting from Telegram text", () => {
  const formatted = normalizeDiscordMarkdown(
    "Ping @everyone @here @some_user email admin@example.com ||secret|| </status:123> <t:0:R>",
  );

  assert.equal(
    formatted,
    "Ping email admin@example.com secret /status 1970-01-01 00:00 UTC",
  );
  assert.doesNotMatch(formatted, /(^|\s)@[A-Za-z0-9_]{5,32}\b/);
  assert.doesNotMatch(formatted, /<[@#:t/]|\|\|/);
});

test("builds Telegram text from content and embeds", () => {
  const payload = buildTelegramCrosspostPayload(route, {
    content: "**Server updated**",
    authorName: "Admin",
    discordUrl: "https://discord.com/channels/1/2/3",
    embeds: [
      {
        title: "Changes",
        description: "Fixed `/home`.",
        url: null,
        fields: [{ name: "Version", value: "1.2.3" }],
      },
    ],
    attachments: [
      {
        filename: "update.png",
        url: "https://cdn.discordapp.com/update.png",
        contentType: "image/png",
      },
    ],
  });

  assert.equal(payload.textChunks.length, 1);
  assert.doesNotMatch(payload.textChunks[0] ?? "", /📣/);
  assert.match(payload.textChunks[0] ?? "", /^Posted by Admin/);
  assert.match(payload.textChunks[0] ?? "", /Fixed \/home\./);
  assert.equal(payload.attachments.length, 1);
});

test("splits long Telegram messages without exceeding the limit", () => {
  const chunks = splitTelegramText("paragraph ".repeat(700));
  assert.ok(chunks.length > 1);
  assert.ok(chunks.every((chunk) => chunk.length <= 4096));
  assert.equal(
    chunks.join(" ").replace(/\s+/g, " ").trim(),
    "paragraph ".repeat(700).trim(),
  );
});
