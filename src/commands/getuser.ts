import {
  type ChatInputCommandInteraction,
  MessageFlags,
  PermissionFlagsBits,
  SlashCommandBuilder,
} from "discord.js";
import type { Command } from "../types/command";
import { getPlayerByDiscordId } from "../utils/helpers";

const GetUserCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("getuser")
    .setDescription("…")
    .addUserOption((option) =>
      option.setName("id").setDescription("…").setRequired(true),
    )
    .setDefaultMemberPermissions(PermissionFlagsBits.Administrator),

  cooldown: 0,
  admin: true,

  async execute(interaction: ChatInputCommandInteraction) {
    const discordUser = interaction.options.getUser("id", true);
    await interaction.deferReply({ flags: MessageFlags.Ephemeral });

    try {
      const info = await getPlayerByDiscordId(discordUser.id);
      if (!info) {
        await interaction.editReply(
          `No Minecraft account linked for ${discordUser.tag}.`,
        );
        return;
      }

      await interaction.editReply(
        `Discord user **${discordUser.tag}** is linked to Minecraft user **${info.name}**.\n` +
          `Top Rank: **${info.topRank}**\n` +
          `First Join Year: **${info.firstJoinYear}**`,
      );
    } catch (error) {
      console.error(error);
      await interaction.editReply(
        "An error occurred while fetching user info.",
      );
    }
  },
};

export default GetUserCommand;
