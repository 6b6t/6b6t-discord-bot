import type { TelegramCrosspostRoute } from "./config";

export type CrosspostEmbed = {
  title: string | null;
  description: string | null;
  url: string | null;
  fields: Array<{ name: string; value: string }>;
};

export type CrosspostAttachment = {
  filename: string;
  url: string;
  contentType: string | null;
};

export type DiscordCrosspostContent = {
  content: string;
  authorName: string;
  discordUrl: string;
  embeds: CrosspostEmbed[];
  attachments: CrosspostAttachment[];
};

export type TelegramCrosspostPayload = {
  textChunks: string[];
  attachments: CrosspostAttachment[];
};

export const TELEGRAM_TEXT_LIMIT = 4096;

function formatDiscordTimestamp(seconds: string) {
  const timestamp = Number.parseInt(seconds, 10);
  const date = new Date(timestamp * 1_000);
  if (!Number.isFinite(timestamp) || Number.isNaN(date.getTime())) return "";
  return `${date.toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

export function normalizeDiscordMarkdown(value: string) {
  return value
    .replace(/<a?:[A-Za-z0-9_]+:\d+>/g, "")
    .replace(/<@!?\d+>/g, "")
    .replace(/<@&\d+>/g, "")
    .replace(/<#\d+>/g, "")
    .replace(/<\/([^:>]+):\d+>/g, "/$1")
    .replace(/<t:(-?\d+)(?::[tTdDfFR])?>/g, (_match, seconds: string) =>
      formatDiscordTimestamp(seconds),
    )
    .replace(/\[([^\]]+)]\((https?:\/\/[^)]+)\)/g, "$1 ($2)")
    .replace(/\|\|([\s\S]*?)\|\|/g, "$1")
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/^>\s?/gm, "")
    .replace(/```(?:[A-Za-z0-9_-]+)?\n?([\s\S]*?)```/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/__([^_]+)__/g, "$1")
    .replace(/~~([^~]+)~~/g, "$1")
    .replace(/(^|\s)[*_]([^*_\n]+)[*_](?=\s|$)/g, "$1$2")
    .replace(/(^|[\s([{:])@(everyone|here)\b/gi, "$1")
    .replace(/(^|[\s([{:])@[A-Za-z0-9_]{5,32}\b/g, "$1")
    .replace(/[ \t]{2,}/g, " ")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

function formatEmbed(embed: CrosspostEmbed) {
  const parts: string[] = [];
  if (embed.title) parts.push(embed.title);
  if (embed.description) parts.push(embed.description);
  for (const field of embed.fields) {
    parts.push(`${field.name}\n${field.value}`);
  }
  if (embed.url && !parts.some((part) => part.includes(embed.url ?? ""))) {
    parts.push(embed.url);
  }
  return normalizeDiscordMarkdown(parts.join("\n\n"));
}

function takeChunk(value: string, limit: number) {
  if (value.length <= limit) return [value, ""] as const;

  const candidate = value.slice(0, limit + 1);
  const paragraphBreak = candidate.lastIndexOf("\n\n");
  const lineBreak = candidate.lastIndexOf("\n");
  const wordBreak = candidate.lastIndexOf(" ");
  const splitAt =
    paragraphBreak >= Math.floor(limit * 0.4)
      ? paragraphBreak
      : lineBreak >= Math.floor(limit * 0.5)
        ? lineBreak
        : wordBreak >= Math.floor(limit * 0.6)
          ? wordBreak
          : limit;

  return [value.slice(0, splitAt).trimEnd(), value.slice(splitAt).trimStart()];
}

export function splitTelegramText(value: string, limit = TELEGRAM_TEXT_LIMIT) {
  if (!value.trim()) return [];
  if (limit < 1) throw new Error("Telegram text limit must be positive");

  const chunks: string[] = [];
  let remaining = value.trim();
  while (remaining.length > 0) {
    const [chunk, rest] = takeChunk(remaining, limit);
    chunks.push(chunk);
    remaining = rest;
  }
  return chunks;
}

export function buildTelegramCrosspostPayload(
  route: TelegramCrosspostRoute,
  message: DiscordCrosspostContent,
): TelegramCrosspostPayload {
  const bodyParts: string[] = [];
  const content = normalizeDiscordMarkdown(message.content);
  if (content) bodyParts.push(content);

  for (const embed of message.embeds) {
    const formatted = formatEmbed(embed);
    if (formatted) bodyParts.push(formatted);
  }

  const textParts: string[] = [];
  if (route.includeAuthor && message.authorName) {
    const safeAuthor = normalizeDiscordMarkdown(message.authorName);
    if (safeAuthor) textParts.push(`Posted by ${safeAuthor}`);
  }

  const body = bodyParts.join("\n\n").trim();
  if (body) textParts.push(body);

  return {
    textChunks: splitTelegramText(textParts.join("\n\n")),
    attachments: message.attachments,
  };
}
