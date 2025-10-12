import { type Interaction, PermissionsBitField } from "discord.js";
import config from "../config/config";
import type { CommandManager } from "../utils/commandManager";

async function handleCommand(
  commandManager: CommandManager,
  interaction: Interaction,
) {
  if (!interaction.isChatInputCommand()) return;

  const command = commandManager.getCommands().get(interaction.commandName);
  const member = interaction.member;
  if (!command) return;
  if (!member) return;

  const isAdmin =
    "cache" in member.roles
      ? member.roles.cache.has(config.commandAdminRoleId)
      : member.roles.includes(config.commandAdminRoleId);

  if (command.admin && !isAdmin) {
    await interaction.reply({
      content: "You do not have permission to use this command.",
      ephemeral: true,
    });
    return;
  }

  const cooldown = command.cooldown ?? 60;
  const remaining = commandManager.isOnCooldown(
    command.data.name,
    interaction.user.id,
    cooldown,
  );
  if (remaining > 0) {
    await interaction.reply({
      content: `You must wait ${remaining}s before using this command again.`,
      ephemeral: true,
    });
    return;
  }

  try {
    await command.execute(interaction);
  } catch (error) {
    console.error(error);
    if (interaction.deferred || interaction.replied) {
      await interaction.editReply(
        "There was an error while executing this command",
      );
    } else {
      await interaction.reply({
        content: "There was an error while executing this command",
        ephemeral: true,
      });
    }
  }
}

async function handleRoleMenu(interaction: Interaction) {
  if (!interaction.isStringSelectMenu()) return;
  if (interaction.customId !== "legend_role_menu") return;

  const member = interaction.guild?.members.cache.get(interaction.user.id);
  if (!member) return;
  if (
    !member.roles.cache.has(config.roleMenuRequiredRoleId) &&
    !member.permissions.has(PermissionsBitField.Flags.Administrator)
  ) {
    await interaction.reply({
      content: `You don't have the <@&${config.roleMenuRequiredRoleId}> role.`,
      ephemeral: true,
    });
    return;
  }

  const roleIds = interaction.values;
  const selectedId = roleIds[0];

  try {
    const menuRoleIds = config.roleMenuRoleIds.filter(
      (id) => id !== selectedId,
    );

    if (selectedId === "clear_top" || selectedId === "clear_bottom") {
      const hasColorRole = member.roles.cache.some((r) =>
        config.roleMenuRoleIds.includes(r.id),
      );

      if (hasColorRole) {
        await member.roles.remove(config.roleMenuRoleIds);
        await interaction.reply({
          content: `Your color role has been removed.`,
          ephemeral: true,
        });
      } else {
        await interaction.reply({
          content: `You don't have any color role.`,
          ephemeral: true,
        });
      }
      return;
    }

    await member.roles.remove(menuRoleIds);
    await member.roles.add(selectedId);

    await interaction.reply({
      content: `You have been given the color: <@&${selectedId}>`,
      ephemeral: true,
    });
  } catch (error) {
    console.error(
      `Error while assigning role ${selectedId} to user ${member.id}: `,
      error,
    );
    await interaction.reply({
      content: `Failed to assign color <@&${selectedId}>`,
      ephemeral: true,
    });
  }
}

export const onInteractionCreate = async (
  commandManager: CommandManager,
  interaction: Interaction,
) => {
  await handleCommand(commandManager, interaction);
  await handleRoleMenu(interaction);
};
