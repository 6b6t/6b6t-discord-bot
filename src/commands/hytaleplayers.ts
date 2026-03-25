import {
  type ChatInputCommandInteraction,
  SlashCommandBuilder,
} from "discord.js";
import type { Command } from "../types/command";
import { getHytalePlayerCountData } from "../utils/helpers";

const HytalePlayersCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("hytaleplayers")
    .setDescription("Check the current 6b6t Hytale player count"),

  cooldown: 0,

  async execute(interaction: ChatInputCommandInteraction) {
    const data = await getHytalePlayerCountData();
    if (!data) {
      await interaction.reply({
        content: "Failed to get Hytale player count.",
        ephemeral: true,
      });
      return;
    }

    let message = `There are currently ${data.playerCount}/${data.maxPlayers} players online on 6b6t Hytale.`;

    if (data.metrics) {
      const { tps, entities, chunks } = data.metrics;
      const metricsParts = [];
      if (tps !== null) metricsParts.push(`**TPS**: ${tps.toFixed(1)}`);
      if (entities !== null) metricsParts.push(`**Entities**: ${entities}`);
      if (chunks !== null) metricsParts.push(`**Chunks**: ${chunks}`);

      if (metricsParts.length > 0) {
        message += `\n${metricsParts.join(" | ")}`;
      }
    }

    if (data.players.length > 0) {
      const names = data.players.map((player) => player.Name).join(", ");
      message += `\n**Online players**: ${names}`;
    }

    await interaction.reply(message);
  },
};

export default HytalePlayersCommand;
