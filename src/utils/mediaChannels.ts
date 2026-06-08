import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import type { Client, TextChannel } from "discord.js";
import config from "../config/config";

const DEFAULT_MEDIA_CHANNEL_FREQUENCY = 3;
const MIN_MEDIA_CHANNEL_FREQUENCY = 1;
const MAX_MEDIA_CHANNEL_FREQUENCY = 100;
const STORAGE_PATH = join(
  join(process.cwd(), "data"),
  "media-channel-settings.json",
);

type MediaChannelSettings = {
  frequency: number;
};

let cachedFrequency: number | null = null;
const messageCounts = new Map<string, number>();

function normalizeChannelName(name: string): string {
  return name.toLowerCase().replace(/^[^a-z0-9]+/, "");
}

function normalizeFrequency(value: unknown): number {
  if (typeof value !== "number" || !Number.isInteger(value)) {
    return DEFAULT_MEDIA_CHANNEL_FREQUENCY;
  }

  return Math.min(
    Math.max(value, MIN_MEDIA_CHANNEL_FREQUENCY),
    MAX_MEDIA_CHANNEL_FREQUENCY,
  );
}

async function readSettings(): Promise<MediaChannelSettings> {
  try {
    const raw = await readFile(STORAGE_PATH, "utf-8");
    const parsed = JSON.parse(raw) as Partial<MediaChannelSettings>;
    return { frequency: normalizeFrequency(parsed.frequency) };
  } catch {
    return { frequency: DEFAULT_MEDIA_CHANNEL_FREQUENCY };
  }
}

async function writeSettings(settings: MediaChannelSettings): Promise<void> {
  await mkdir(dirname(STORAGE_PATH), { recursive: true });
  await writeFile(STORAGE_PATH, JSON.stringify(settings, null, 2));
}

export async function getMediaChannelFrequency(): Promise<number> {
  if (cachedFrequency !== null) {
    return cachedFrequency;
  }

  const settings = await readSettings();
  cachedFrequency = settings.frequency;
  return cachedFrequency;
}

export async function setMediaChannelFrequency(frequency: number) {
  const normalizedFrequency = normalizeFrequency(frequency);
  await writeSettings({ frequency: normalizedFrequency });
  cachedFrequency = normalizedFrequency;
}

export function isMediaChannel(channel: TextChannel): boolean {
  const channelName = normalizeChannelName(channel.name);
  return config.mediaChannelNames.some(
    (name) => normalizeChannelName(name) === channelName,
  );
}

export function getMediaChannelFrequencyBounds() {
  return {
    min: MIN_MEDIA_CHANNEL_FREQUENCY,
    max: MAX_MEDIA_CHANNEL_FREQUENCY,
  };
}

export function shouldSendMediaChannelReminder(
  channelId: string,
  frequency: number,
): boolean {
  const count = (messageCounts.get(channelId) ?? 0) + 1;

  if (count >= frequency) {
    messageCounts.set(channelId, 0);
    return true;
  }

  messageCounts.set(channelId, count);
  return false;
}

export async function deleteLatestMediaChannelReminder(
  client: Client,
  channel: TextChannel,
  limit: number = 100,
) {
  const messages = await channel.messages.fetch({ limit });
  const botReminder = messages.find(
    (msg) =>
      msg.author.id === client.user?.id &&
      msg.content === config.mediaChannelMessage,
  );

  if (!botReminder) return;

  try {
    await botReminder.delete();
  } catch (error) {
    console.error(
      `Failed to delete media channel reminder in ${channel.id}: `,
      error,
    );
  }
}
