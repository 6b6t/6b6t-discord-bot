import type { ChatInputCommandInteraction } from "discord.js";
import { SlashCommandBuilder } from "discord.js";
import type { Command } from "../types/command";
import { formatDuration, getServerData } from "../utils/helpers";

const PlayerCountCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("playercount")
    .setDescription("See 6b6t's current player count and uptime"),

  async execute(interaction: ChatInputCommandInteraction) {
    const data = await getServerData();
    if (!data) {
      await interaction.reply({
        content: "Failed to get server data",
        ephemeral: true,
      });
      return;
    }

    let uptimeStr: string | null = null;
    if (data.uptime?.serverStartUnix) {
      const diff = Math.floor(Date.now() / 1000 - data.uptime.serverStartUnix);
      uptimeStr = formatDuration(diff);
    } else if (data.uptime?.currentUptimeHours) {
      const totalSeconds = Math.floor(data.uptime.currentUptimeHours * 3600);
      uptimeStr = formatDuration(totalSeconds);
    }

    if (!uptimeStr) {
      await interaction.reply("The server is currently down.");
      return;
    }

    await interaction.reply(
      `There are currently ${data.playerCount} players online on 6b6t. The server has been up for ${uptimeStr}.`,
    );
  },
};

export default PlayerCountCommand;
