import {
  ButtonInteraction,
  CommandInteraction,
  ModalSubmitInteraction,
} from 'discord.js';

export interface Command {
  data: any;
  cooldown?: number;
  execute: (interaction: CommandInteraction) => Promise<void>;
  handleButton?: (interaction: ButtonInteraction) => Promise<void>;
  handleModal?: (interaction: ModalSubmitInteraction) => Promise<void>;
}
