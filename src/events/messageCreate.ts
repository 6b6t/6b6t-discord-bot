import {
  ChannelType,
  type Client,
  type Message,
  MessageFlags,
  type TextChannel,
} from "discord.js";
import config from "../config/config";
import { deleteLatestMessage } from "../utils/helpers";
import {
  deleteLatestMediaChannelReminder,
  getMediaChannelFrequency,
  isMediaChannel,
  shouldSendMediaChannelReminder,
} from "../utils/mediaChannels";

async function handleChannel(
  client: Client,
  channel: TextChannel,
  message: Message,
  channelId: string,
  content: string,
) {
  if (channel.type !== ChannelType.GuildText) return;
  if (channel.id !== channelId) return;
  if (message.author.bot) return;

  await deleteLatestMessage(client, channel);

  try {
    await channel.send({
      content,
      allowedMentions: { users: [], roles: [], repliedUser: false },
      flags: [MessageFlags.SuppressNotifications],
    });
  } catch (error) {
    console.error(`Could not send message in channel (${channelId}): `, error);
  }
}

async function handleMediaChannel(
  client: Client,
  channel: TextChannel,
  message: Message,
) {
  if (channel.type !== ChannelType.GuildText) return;
  if (!isMediaChannel(channel)) return;
  if (message.author.bot) return;

  const frequency = await getMediaChannelFrequency();
  if (!shouldSendMediaChannelReminder(channel.id, frequency)) return;

  await deleteLatestMediaChannelReminder(client, channel);

  try {
    await channel.send({
      content: config.mediaChannelMessage,
      allowedMentions: { users: [], roles: [], repliedUser: false },
      flags: [MessageFlags.SuppressNotifications],
    });
  } catch (error) {
    console.error(
      `Could not send media channel reminder in channel (${channel.id}): `,
      error,
    );
  }
}

export const onMessageCreate = async (client: Client, message: Message) => {
  if (message.channel.type !== ChannelType.GuildText) return;

  if (message.channel.id === config.updatesId) {
    try {
      if (!message.author.bot) {
        await message.author.send(
          "Please post your announcement on r/6b6t subreddit too.",
        );
      }
    } catch (error) {
      console.error(
        `[UpdatesDM] Failed to DM user ${message.author.id}:`,
        error,
      );
    }
  }

  await handleChannel(
    client,
    message.channel,
    message,
    config.advertisingId,
    config.advertisingMessage,
  );
  await handleChannel(
    client,
    message.channel,
    message,
    config.merchId,
    config.merchMessage,
  );
  await handleMediaChannel(client, message.channel, message);
};
