import {
  ChannelType,
  type Client,
  type Message,
  MessageFlags,
  type TextChannel,
} from "discord.js";
import config from "../config/config";
import { deleteLatestMessage } from "../utils/helpers";

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

export const onMessageCreate = async (client: Client, message: Message) => {
  if (message.channel.type !== ChannelType.GuildText) return;
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
};
