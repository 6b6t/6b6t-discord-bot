import {
  ActionRowBuilder,
  BaseGuildTextChannel,
  ComponentType,
  EmbedBuilder,
  StringSelectMenuBuilder,
  StringSelectMenuOptionBuilder,
} from 'discord.js';
import { Guild, Role } from 'discord.js';
import config from '../config/config';

function buildRoleMenu(roleIds: string[], guild: Guild): ActionRowBuilder {
  const roles: Role[] = roleIds
    .map((id) => guild.roles.cache.get(id))
    .filter((r): r is Role => !!r);

  const selectMenu = new StringSelectMenuBuilder()
    .setCustomId('legend_role_menu')
    .setPlaceholder('Select a color')
    .addOptions(
      ...roles.map((role) =>
        new StringSelectMenuOptionBuilder()
          .setLabel(role.name)
          .setValue(role.id),
      ),
    );

  return new ActionRowBuilder().addComponents(selectMenu);
}

export async function existsRoleMenu(
  channel: BaseGuildTextChannel,
): Promise<boolean> {
  const messages = await channel.messages.fetch({ limit: 10 });

  for (const message of messages.values()) {
    for (const row of message.components) {
      if (row.type !== ComponentType.ActionRow) continue;
      if (!row.components) continue;

      for (const component of row.components) {
        if (
          component.type === ComponentType.StringSelect &&
          component.customId === 'legend_role_menu'
        ) {
          return true;
        }
      }
    }
  }

  return false;
}

export async function sendRoleMenu(channel: BaseGuildTextChannel) {
  const row: ActionRowBuilder = buildRoleMenu(
    config.roleMenuRoleIds,
    channel.guild,
  );

  const embed = new EmbedBuilder()
    .setTitle('Legend Color Roles')
    .setDescription(
      'Change your color in the Discord by picking one of the colors.',
    )
    .setThumbnail('https://www.6b6t.org/_next/image?url=%2Flogo.png&w=48&q=75')
    .setColor('#FFF11A');

  try {
    await channel.send({
      embeds: [embed],
      components: [row.toJSON()],
    });
  } catch (error) {
    console.error(
      `Failed to send role menu message in channel ${channel.id} (${channel}): `,
      error,
    );
  }
}
