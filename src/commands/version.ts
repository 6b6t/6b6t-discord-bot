import { SlashCommandBuilder, CommandInteraction } from 'discord.js';
import { Command } from '../types/command';
import config from '../config/config';

const VersionCommand: Command = {
  data: new SlashCommandBuilder()
    .setName('version')
    .setDescription("See 6b6t's version"),

  async execute(interaction: CommandInteraction) {
    await interaction.reply(
      `The current version of 6b6t is ${config.serverVersion}. Connect to 6b6t using the IP \`play.6b6t.org\` on Java Edition and \`bedrock.6b6t.org\` with the port 19132 on Bedrock Edition.`,
    );
  },
};

export default VersionCommand;
