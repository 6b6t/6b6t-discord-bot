import { SlashCommandBuilder, CommandInteraction } from 'discord.js';
import { Command } from '../types/command';

const IpCommand: Command = {
  data: new SlashCommandBuilder().setName('ip').setDescription("See 6b6t's IP"),

  async execute(interaction: CommandInteraction) {
    await interaction.reply(
      `Join 6b6t using the IP \`play.6b6t.org\` on Java Edition and \`bedrock.6b6t.org\` with the port 19132 on Bedrock Edition.`,
    );
  },
};

export default IpCommand;
