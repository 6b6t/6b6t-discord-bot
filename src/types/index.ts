import type {
  ChatInputCommandInteraction,
  ClientEvents,
  SlashCommandBuilder,
} from "discord.js";

export interface Command {
  data: SlashCommandBuilder;
  execute: (interaction: ChatInputCommandInteraction) => Promise<void>;
}

export interface Event<K extends keyof ClientEvents = keyof ClientEvents> {
  name: K;
  execute: (...args: ClientEvents[K]) => void;
}
