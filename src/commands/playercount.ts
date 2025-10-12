import { type CommandInteraction, SlashCommandBuilder } from "discord.js";
import config from "../config/config";
import type { Command } from "../types/command";
import { getServerData } from "../utils/helpers";

const PlayerCountCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("playercount")
    .setDescription("See 6b6t's current player count"),

  async execute(interaction: CommandInteraction) {
    const data = await getServerData(config.statusHost);
    if (!data) {
      await interaction.reply({
        content: "Failed to get server data",
        ephemeral: true,
      });
    }

    await interaction.reply(
      `There are currently ${data.players.now} players online on 6b6t.`,
    );
  },
};

export default PlayerCountCommand;
