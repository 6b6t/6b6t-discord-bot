import { type CommandInteraction, SlashCommandBuilder } from "discord.js";
import config from "../config/config";
import type { Command } from "../types/command";
import { getServerData } from "../utils/helpers";

const PlayerCountCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("playercount")
    .setDescription("See 6b6t's current player count and uptime"),

  async execute(interaction: CommandInteraction) {
    const data = await getServerData(config.statusHost);
    if (!data) {
      await interaction.reply({
        content: "Failed to get server data",
        ephemeral: true,
      });
      return;
    }

    let uptimeStr = "";
    if (data.uptime.serverStartUnix) {
      const diff = Math.floor(Date.now() / 1000 - data.uptime.serverStartUnix);
      const days = Math.floor(diff / 86400);
      const hours = Math.floor((diff % 86400) / 3600);
      const minutes = Math.floor((diff % 3600) / 60);
      const seconds = diff % 60;
      const parts = [];
      if (days > 0) parts.push(`${days}d`);
      if (hours > 0) parts.push(`${hours}h`);
      if (minutes > 0) parts.push(`${minutes}m`);
      if (seconds > 0) parts.push(`${seconds}s`);
      uptimeStr = parts.join(" ");
    } else if (data.uptime.currentUptimeHours) {
      const totalSeconds = Math.floor(data.uptime.currentUptimeHours * 3600);
      const days = Math.floor(totalSeconds / 86400);
      const hours = Math.floor((totalSeconds % 86400) / 3600);
      const minutes = Math.floor((totalSeconds % 3600) / 60);
      const seconds = totalSeconds % 60;
      const parts = [];
      if (days > 0) parts.push(`${days}d`);
      if (hours > 0) parts.push(`${hours}h`);
      if (minutes > 0) parts.push(`${minutes}m`);
      if (seconds > 0) parts.push(`${seconds}s`);
      uptimeStr = parts.join(" ");
    } else {
      await interaction.reply(`The server is currently down.`);
      return;
    }

    await interaction.reply(
      `There are currently ${data.players.online} players online on 6b6t. The server has been up for ${uptimeStr}.`,
    );
  },
};

export default PlayerCountCommand;
