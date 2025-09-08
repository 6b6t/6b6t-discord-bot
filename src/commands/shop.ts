import { SlashCommandBuilder, CommandInteraction } from 'discord.js';
import { Command } from '../types/command';

const ShopCommand: Command = {
  data: new SlashCommandBuilder()
    .setName('shop')
    .setDescription("See 6b6t's shop"),

  async execute(interaction: CommandInteraction) {
    await interaction.reply(
      `You can support 6b6t financially and get benefits like more homes, lower /tpa and /home cooldowns, access to commands like /hat, /balloons, /chatcolor and much more at the [6b6t Shop](<https://www.6b6t.org/?utm_source=discord&utm_medium=discord_bot_command&utm_campaign=discord_bot_command_shop>).`,
    );
  },
};

export default ShopCommand;
