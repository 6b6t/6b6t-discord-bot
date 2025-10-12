import type {
  Client,
  MessageReaction,
  PartialMessageReaction,
  PartialUser,
  User,
} from "discord.js";
import config from "../config/config";
import { handleReactionRemove } from "../utils/reactionMenu";

export const onMessageReactionRemove = async (
  _client: Client,
  reaction: MessageReaction | PartialMessageReaction,
  user: User | PartialUser,
) => {
  if (user.bot) return;

  const message = reaction.message.partial
    ? await reaction.message.fetch()
    : reaction.message;

  const emoji = reaction.emoji.name;
  if (!emoji) return;

  const roles = [
    ...config.languageMenuRoleIds,
    ...config.notificationMenuRoleIds,
  ];

  await handleReactionRemove(message, emoji, user.id, roles);
};
