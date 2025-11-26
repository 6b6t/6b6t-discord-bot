import { type CommandInteraction, SlashCommandBuilder } from "discord.js";
import type { Command } from "../types/command";

const ShopCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("shop")
    .setDescription("See 6b6t's shop"),

  async execute(interaction: CommandInteraction) {
    await interaction.reply(
      `You can support 6b6t financially and get benefits like more homes, lower /tpa and /home cooldowns, access to commands like /hat, /balloons, /chatcolor and much more at the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_command&utm_campaign=evergreen_shop&utm_content=shop&lang=en>).`,
    );
  },
};

export default ShopCommand;
