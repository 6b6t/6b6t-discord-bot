import type {
  ButtonInteraction,
  ChatInputCommandInteraction,
  ModalSubmitInteraction,
  SlashCommandBuilder,
  SlashCommandOptionsOnlyBuilder,
  SlashCommandSubcommandsOnlyBuilder,
} from "discord.js";

export interface Command {
  data:
    | SlashCommandBuilder
    | SlashCommandSubcommandsOnlyBuilder
    | SlashCommandOptionsOnlyBuilder;
  cooldown?: number;
  admin?: boolean;
  execute: (interaction: ChatInputCommandInteraction) => Promise<void>;
  handleButton?: (interaction: ButtonInteraction) => Promise<void>;
  handleModal?: (interaction: ModalSubmitInteraction) => Promise<void>;
}
