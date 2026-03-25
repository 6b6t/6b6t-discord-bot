import {
  type ChatInputCommandInteraction,
  MessageFlags,
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
    await interaction.deferReply({ flags: MessageFlags.Ephemeral });

    const data = await getHytalePlayerCountData();
    if (!data) {
      await interaction.editReply("Failed to get Hytale player count.");
      return;
    }

    let message = `There are currently ${data.playerCount}/${data.maxPlayers} players online on 6b6t Hytale.`;

    if (data.players.length > 0) {
      const names = data.players.map((player) => player.Name).join(", ");
      message += ` Online players: ${names}`;
    }

    await interaction.editReply(message);
  },
};

export default HytalePlayersCommand;
