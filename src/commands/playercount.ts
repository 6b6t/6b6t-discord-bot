import { type CommandInteraction, SlashCommandBuilder } from "discord.js";
import type { Command } from "../types/command";
import { getServerData } from "../utils/helpers";

const PlayerCountCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("playercount")
    .setDescription("See 6b6t's current player count"),

  async execute(interaction: CommandInteraction) {
    const data = await getServerData();
    if (!data) {
      await interaction.reply({
        content: "Failed to get server data",
        ephemeral: true,
      });
      return;
    }

    await interaction.reply(
      `There are currently ${data.playerCount} players online on 6b6t.`,
    );
  },
};

export default PlayerCountCommand;
