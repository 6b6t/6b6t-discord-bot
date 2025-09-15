import {
  SlashCommandBuilder,
  CommandInteraction,
  MessageFlags,
} from 'discord.js';
import { Command } from '../types/command';

const BoostCommand: Command = {
  data: new SlashCommandBuilder()
    .setName('boost')
    .setDescription('See the perks for boosting the 6b6t Discord'),

  async execute(interaction: CommandInteraction) {
    await interaction.reply({
      content: `Boosting the 6b6t Discord gives you the <@&933418896692768820> role, nickname changing permissions, embed, media and emoji permissions in ⁠general and free access to priority support in the Discord channel #⁠📜premium-tickets.`,
      allowedMentions: {
        users: [],
        roles: [],
        repliedUser: false,
      },
      flags: [MessageFlags.SuppressNotifications],
    });
  },
};

export default BoostCommand;
