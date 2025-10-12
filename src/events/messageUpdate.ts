import {
  ChannelType,
  type Client,
  type Message,
  type PartialMessage,
} from "discord.js";
import config from "../config/config";

export const onMessageUpdate = async (
  _client: Client,
  _oldMessage: Message | PartialMessage,
  newMessage: Message | PartialMessage,
) => {
  if (!(newMessage.channel.type === ChannelType.GuildText)) return;
  if (newMessage.channel.id !== config.reviewId) return;
  if (!newMessage.author) return;
  if (newMessage.author.bot) return;

  const member = newMessage.member;
  if (!member) return;

  if (member.roles.cache.some((r) => config.reviewIgnoreRoleIds.includes(r.id)))
    return; // ignore user with any of these roles
  try {
    await newMessage.delete();
  } catch (error) {
    console.error(
      `Could not delete review message (user ${member.id}): `,
      error,
    );
  }
};
