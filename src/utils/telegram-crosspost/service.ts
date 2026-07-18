import crypto from "node:crypto";
import type {
  Client,
  Message,
  PartialMessage,
  TextBasedChannel,
} from "discord.js";
import {
  getTelegramCrosspostConfig,
  type TelegramCrosspostConfig,
  type TelegramCrosspostRoute,
} from "./config";
import {
  buildTelegramCrosspostPayload,
  type DiscordCrosspostContent,
  type TelegramCrosspostPayload,
} from "./formatter";
import {
  beginCrosspost,
  ensureCrosspostStorage,
  getCrosspostRecord,
  getRecoverableCrossposts,
  getRouteCheckpoint,
  markCrosspostDeleted,
  markCrosspostFailed,
  markCrosspostSent,
  setRouteCheckpoint,
} from "./storage";
import { TelegramClient, TelegramDeliveryError } from "./telegram-client";

type HydratedMessage = Message<true>;

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function compareSnowflakes(left: string, right: string) {
  try {
    const leftValue = BigInt(left);
    const rightValue = BigInt(right);
    return leftValue < rightValue ? -1 : leftValue > rightValue ? 1 : 0;
  } catch {
    return left.localeCompare(right);
  }
}

function removeMentions(value: string) {
  return value
    .replace(/<@!?\d+>/g, "")
    .replace(/<@&\d+>/g, "")
    .replace(/<#\d+>/g, "");
}

function extractMessageContent(
  message: HydratedMessage,
): DiscordCrosspostContent {
  const attachments = message.attachments.map((attachment) => ({
    filename: attachment.name,
    url: attachment.url,
    contentType: attachment.contentType,
  }));
  const attachmentUrls = new Set(attachments.map((item) => item.url));

  for (const [index, embed] of message.embeds.entries()) {
    for (const [kind, url] of [
      ["image", embed.image?.url],
      ["thumbnail", embed.thumbnail?.url],
    ] as const) {
      if (!url || attachmentUrls.has(url)) continue;
      attachmentUrls.add(url);
      let filename = `embed-${kind}-${index + 1}`;
      try {
        filename = new URL(url).pathname.split("/").pop() || filename;
      } catch {
        // Keep the deterministic fallback filename.
      }
      attachments.push({ filename, url, contentType: null });
    }
  }

  return {
    content: removeMentions(message.content),
    authorName: message.author.globalName ?? message.author.username,
    discordUrl: message.url,
    embeds: message.embeds.map((embed) => ({
      title: embed.title,
      description: embed.description ? removeMentions(embed.description) : null,
      url: embed.url,
      fields: embed.fields.map((field) => ({
        name: removeMentions(field.name),
        value: removeMentions(field.value),
      })),
    })),
    attachments,
  };
}

function stableAttachmentUrl(value: string) {
  try {
    const url = new URL(value);
    return `${url.origin}${url.pathname}`;
  } catch {
    return value.split("?", 1)[0] ?? value;
  }
}

function contentHash(value: TelegramCrosspostPayload) {
  const stableValue = {
    ...value,
    attachments: value.attachments.map((attachment) => ({
      ...attachment,
      url: stableAttachmentUrl(attachment.url),
    })),
  };
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(stableValue))
    .digest("hex");
}

async function hydrateMessage(message: Message | PartialMessage) {
  const hydrated = message.partial ? await message.fetch() : message;
  return hydrated.inGuild() ? (hydrated as HydratedMessage) : null;
}

async function fetchDiscordMessage(
  client: Client,
  channelId: string,
  messageId: string,
) {
  const channel = await client.channels.fetch(channelId);
  if (!channel?.isTextBased() || !("messages" in channel)) return null;
  try {
    const message = await channel.messages.fetch(messageId);
    return message.inGuild() ? (message as HydratedMessage) : null;
  } catch {
    return null;
  }
}

export class TelegramCrosspostService {
  private config: TelegramCrosspostConfig | null = null;
  private telegram: TelegramClient | null = null;
  private client: Client | null = null;
  private queueTail: Promise<void> = Promise.resolve();
  private readonly scheduledJobs = new Set<string>();

  private get routes() {
    return this.config?.routes ?? [];
  }

  private routesForChannel(channelId: string) {
    return this.routes.filter((route) => route.discordChannelId === channelId);
  }

  private enqueue(key: string, job: () => Promise<void>) {
    if (this.scheduledJobs.has(key)) return;
    this.scheduledJobs.add(key);
    this.queueTail = this.queueTail
      .then(job)
      .catch((error) => {
        console.error(`[TelegramCrosspost] Job ${key} failed:`, error);
      })
      .finally(() => {
        this.scheduledJobs.delete(key);
      });
  }

  async onReady(client: Client) {
    this.client = client;
    try {
      this.config = getTelegramCrosspostConfig();
    } catch (error) {
      console.error("[TelegramCrosspost] Invalid configuration:", error);
      return;
    }

    if (!this.config.enabled || !this.config.token) {
      console.log("[TelegramCrosspost] Disabled: no routes configured");
      return;
    }

    try {
      await ensureCrosspostStorage();
      this.telegram = new TelegramClient(this.config.token, {
        retryAttempts: this.config.retryAttempts,
      });
      const bot = await this.telegram.verifyConnection();
      console.log(
        `[TelegramCrosspost] Connected as ${bot.username ?? bot.id}; routes=${this.routes.length}`,
      );
    } catch (error) {
      console.error("[TelegramCrosspost] Initialization failed:", error);
      this.telegram = null;
      return;
    }

    this.enqueue("startup-recovery", async () => {
      await this.recoverIncompleteDeliveries();
      await this.backfillMissedMessages();
    });
  }

  async onMessageCreate(message: Message) {
    if (!this.telegram || !message.inGuild()) return;
    if (message.author.id === this.client?.user?.id) return;

    for (const route of this.routesForChannel(message.channelId)) {
      this.enqueue(`create:${route.id}:${message.id}`, async () => {
        const hydrated = await hydrateMessage(message);
        if (hydrated) await this.deliver(route, hydrated, false);
      });
    }
  }

  async onMessageUpdate(message: Message | PartialMessage) {
    if (!this.telegram || !this.config?.syncEdits) return;

    for (const route of this.routesForChannel(message.channelId)) {
      this.enqueue(`update:${route.id}:${message.id}`, async () => {
        const hydrated = await hydrateMessage(message);
        if (hydrated) await this.deliver(route, hydrated, true);
      });
    }
  }

  async onMessageDelete(message: Message | PartialMessage) {
    if (!this.telegram || !this.config?.syncDeletes) return;

    for (const route of this.routesForChannel(message.channelId)) {
      this.enqueue(`delete:${route.id}:${message.id}`, async () => {
        const record = await getCrosspostRecord(route.id, message.id);
        if (!record || record.status === "deleted") return;
        await this.telegram?.deleteMessages(
          record.telegramChatId,
          record.telegramMessages,
        );
        await markCrosspostDeleted(route.id, message.id);
        console.log(
          `[TelegramCrosspost] Deleted route=${route.id} message=${message.id}`,
        );
      });
    }
  }

  private async deliver(
    route: TelegramCrosspostRoute,
    message: HydratedMessage,
    isUpdate: boolean,
  ) {
    if (!this.telegram) return;

    const content = extractMessageContent(message);
    const payload = buildTelegramCrosspostPayload(route, content);
    const hash = contentHash(payload);
    const existing = await getCrosspostRecord(route.id, message.id);

    if (existing?.status === "deleted") return;
    if (existing?.status === "sent" && existing.contentHash === hash) return;
    if (isUpdate && !existing) {
      console.warn(
        `[TelegramCrosspost] Update arrived before create; delivering route=${route.id} message=${message.id}`,
      );
    }

    await beginCrosspost({
      routeId: route.id,
      discordMessageId: message.id,
      discordChannelId: message.channelId,
      telegramChatId: route.telegramChatId,
      contentHash: hash,
    });

    if (existing?.telegramMessages.length) {
      try {
        await this.telegram.deleteMessages(
          existing.telegramChatId,
          existing.telegramMessages,
        );
      } catch (error) {
        await markCrosspostFailed(route.id, message.id, errorMessage(error), [
          ...existing.telegramMessages,
        ]);
        throw error;
      }
    }

    try {
      const sent = await this.telegram.sendPost(
        route.telegramChatId,
        route.telegramThreadId,
        payload,
      );
      await markCrosspostSent(route.id, message.id, sent);
      await this.advanceCheckpoint(route, message.id);
      console.log(
        `[TelegramCrosspost] ${isUpdate ? "Updated" : "Sent"} route=${route.id} message=${message.id} telegramMessages=${sent.length}`,
      );
    } catch (error) {
      const partialMessages =
        error instanceof TelegramDeliveryError ? error.deliveredMessages : [];
      await markCrosspostFailed(
        route.id,
        message.id,
        errorMessage(error),
        partialMessages,
      );
      throw error;
    }
  }

  private async advanceCheckpoint(
    route: TelegramCrosspostRoute,
    messageId: string,
  ) {
    const current = await getRouteCheckpoint(route.id);
    if (!current || compareSnowflakes(current, messageId) < 0) {
      await setRouteCheckpoint(route.id, messageId);
    }
  }

  private async recoverIncompleteDeliveries() {
    if (!this.client || !this.telegram) return;
    const records = await getRecoverableCrossposts();
    for (const record of records) {
      const route = this.routes.find((item) => item.id === record.routeId);
      if (!route) {
        console.warn(
          `[TelegramCrosspost] Cannot recover unknown route=${record.routeId}`,
        );
        continue;
      }

      const message = await fetchDiscordMessage(
        this.client,
        record.discordChannelId,
        record.discordMessageId,
      );
      if (!message) {
        if (record.telegramMessages.length) {
          await this.telegram.deleteMessages(
            record.telegramChatId,
            record.telegramMessages,
          );
        }
        await markCrosspostDeleted(record.routeId, record.discordMessageId);
        continue;
      }

      await this.deliver(route, message, false);
    }
  }

  private async fetchMessagesAfter(channel: TextBasedChannel, after: string) {
    if (!("messages" in channel)) return [];
    const messages: HydratedMessage[] = [];
    let cursor = after;

    while (true) {
      const page = await channel.messages.fetch({
        after: cursor,
        limit: 100,
      });
      const ordered = [...page.values()]
        .filter((message) => message.inGuild())
        .map((message) => message as HydratedMessage)
        .sort((left, right) => compareSnowflakes(left.id, right.id));
      if (ordered.length === 0) break;
      messages.push(...ordered);
      cursor = ordered[ordered.length - 1]?.id ?? cursor;
      if (page.size < 100) break;
    }

    return messages;
  }

  private async backfillMissedMessages() {
    if (!this.client) return;

    for (const route of this.routes) {
      const channel = await this.client.channels.fetch(route.discordChannelId);
      if (!channel?.isTextBased() || !("messages" in channel)) {
        console.error(
          `[TelegramCrosspost] Discord channel is unavailable for route=${route.id}`,
        );
        continue;
      }

      const checkpoint = await getRouteCheckpoint(route.id);
      if (!checkpoint) {
        const latest = await channel.messages.fetch({ limit: 1 });
        const latestMessage = latest.first();
        if (!latestMessage) continue;

        if (this.config?.backfillOnFirstRun && latestMessage.inGuild()) {
          await this.deliver(route, latestMessage as HydratedMessage, false);
        } else {
          await setRouteCheckpoint(route.id, latestMessage.id);
          console.log(
            `[TelegramCrosspost] Initialized route=${route.id} without historical backfill`,
          );
        }
        continue;
      }

      const missed = await this.fetchMessagesAfter(channel, checkpoint);
      for (const message of missed) {
        if (message.author.id === this.client.user?.id) continue;
        await this.deliver(route, message, false);
      }
      if (missed.length > 0) {
        console.log(
          `[TelegramCrosspost] Recovered ${missed.length} missed message(s) for route=${route.id}`,
        );
      }
    }
  }

  async shutdown() {
    await this.queueTail;
  }
}

export const telegramCrosspostService = new TelegramCrosspostService();
