import { CommandInteraction, ButtonInteraction, ModalSubmitInteraction } from 'discord.js';

export interface Command {
    data: any;
    execute: (interaction: CommandInteraction) => Promise<void>;
    handleButton?: (interaction: ButtonInteraction) => Promise<void>;
    handleModal?: (interaction: ModalSubmitInteraction) => Promise<void>;
}
