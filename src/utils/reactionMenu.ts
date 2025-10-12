import type { BaseGuildTextChannel, EmbedBuilder, Message } from "discord.js";

export interface ReactionRole {
  emoji: string;
  id: string;
}

export async function sendReactionRoleMenu(
  channel: BaseGuildTextChannel,
  reactionRoles: ReactionRole[],
  embed: EmbedBuilder,
): Promise<Message | null> {
  try {
    const message = await channel.send({ embeds: [embed] });
    for (const { emoji } of reactionRoles) await message.react(emoji);
    return message;
  } catch (error) {
    console.error(
      `Failed to send reaction role menu in channel ${channel.id}:`,
      error,
    );
    return null;
  }
}

export async function handleReactionAdd(
  message: Message,
  emoji: string,
  userId: string,
  reactionRoles: ReactionRole[],
) {
  const reactionRole = reactionRoles.find((r) => r.emoji === emoji);
  if (!reactionRole) return;

  const member = await message.guild?.members.fetch(userId);
  if (!member) return;

  const role = message.guild?.roles.cache.get(reactionRole.id);
  if (!role) return;

  try {
    await member.roles.add(role);
  } catch (error) {
    console.error(`Failed to add role ${role.id} to user ${userId}: `, error);
  }
}

export async function handleReactionRemove(
  message: Message,
  emoji: string,
  userId: string,
  reactionRoles: ReactionRole[],
) {
  const reactionRole = reactionRoles.find((r) => r.emoji === emoji);
  if (!reactionRole) return;

  const member = await message.guild?.members.fetch(userId);
  if (!member) return;

  const role = message.guild?.roles.cache.get(reactionRole.id);
  if (!role) return;

  try {
    await member.roles.remove(role);
  } catch (error) {
    console.error(
      `Failed to remove role ${role.id} from user ${userId}: `,
      error,
    );
  }
}
