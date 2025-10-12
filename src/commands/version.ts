import { type CommandInteraction, SlashCommandBuilder } from "discord.js";
import config from "../config/config";
import type { Command } from "../types/command";
import { getServerData } from "../utils/helpers";

const VersionCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("version")
    .setDescription("See 6b6t's version"),

  async execute(interaction: CommandInteraction) {
    const data = await getServerData(config.statusHost);
    if (!data) {
      await interaction.reply({
        content: "Failed to get server data",
        ephemeral: true,
      });
    }

    await interaction.reply(
      `The current version of 6b6t is ${data.version}. Connect to 6b6t using the IP \`play.6b6t.org\` on Java Edition and \`bedrock.6b6t.org\` with the port 19132 on Bedrock Edition.`,
    );
  },
};

export default VersionCommand;
