import { CommandInteraction, ButtonInteraction, ModalSubmitInteraction } from 'discord.js';
import { RedisManager } from '../utils/redisManager';

export interface Command {
    data: any;
    execute: (interaction: CommandInteraction, redisManager: RedisManager) => Promise<void>;
    handleButton?: (interaction: ButtonInteraction) => Promise<void>;
    handleModal?: (interaction: ModalSubmitInteraction, redisManager: RedisManager) => Promise<void>;
} 