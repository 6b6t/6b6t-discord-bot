import {
  Client,
  MessageReaction,
  PartialMessageReaction,
  PartialUser,
  User,
} from 'discord.js';
import config from '../config/config';
import { handleReactionAdd } from '../utils/reactionMenu';

export const onMessageReactionAdd = async (
  client: Client,
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

  await handleReactionAdd(message, emoji, user.id, roles);
};
