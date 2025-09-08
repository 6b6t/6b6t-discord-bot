import { Interaction } from 'discord.js';
import { CommandManager } from '../utils/commandManager';

export const onInteractionCreate = async (
  commandManager: CommandManager,
  interaction: Interaction,
) => {
  if (!interaction.isChatInputCommand()) return;

  const command = commandManager.getCommands().get(interaction.commandName);
  if (!command) return;

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
        'There was an error while executing this command',
      );
    } else {
      await interaction.reply({
        content: 'There was an error while executing this command',
        ephemeral: true,
      });
    }
  }
};
