import { SlashCommandBuilder } from '@discordjs/builders';
import { randomBytes } from 'crypto';
import { 
    CommandInteraction,
    EmbedBuilder, 
    ActionRowBuilder, 
    ButtonBuilder, 
    ButtonStyle,
    ModalBuilder,
    TextInputBuilder,
    TextInputStyle,
    ButtonInteraction,
    ModalSubmitInteraction,
    ChannelType,
    TextChannel
} from 'discord.js';
import { RedisManager } from '../utils/redisManager';
import { DatabaseManager } from '../utils/databaseManager';
import { Command } from '../types/command';
import config from '../config/config';

const ALLOWED_CHARS = 'ABCDEFGHJKLMNPQRSTUVWXYZ';

function generateSecureCode(): string {
    const sections = [4, 4, 4, 4];
    const result = sections.map(length => {
        let section = '';
        while (section.length < length) {
            const randomByte = randomBytes(1)[0];
            const index = randomByte % ALLOWED_CHARS.length;
            section += ALLOWED_CHARS[index];
        }
        return section;
    });
    return result.join('-');
}

const command: Command = {
    data: new SlashCommandBuilder()
        .setName('link')
        .setDescription('Link your Minecraft account with Discord')
        .addChannelOption(option => 
            option.setName('channel')
                .setDescription('The channel to send the link message in')
                .setRequired(true)
                .addChannelTypes(ChannelType.GuildText)
        ),

    async execute(interaction: CommandInteraction, redisManager: RedisManager) {
        if (!config.allowedUsers.includes(interaction.user.id)) {
            await interaction.reply({ content: 'You do not have permission to use this command.', ephemeral: true });
            return;
        }

        const channel = interaction.options.get('channel')?.channel as TextChannel;
        if (!channel || !channel.isTextBased()) {
            await interaction.reply({ content: 'Please provide a valid text channel!', ephemeral: true });
            return;
        }

        const embed = new EmbedBuilder()
            .setColor('#0099ff')
            .setTitle('Link Your Minecraft Account')
            .setDescription('Use the command shown below in-game to link your accounts.\n\nLinking will automatically give you all ranks that you purchased from [6b6t Shop](https://6b6t.org/shop).')
            .addFields(
                { name: 'How to link', value: `1. Click the button below to get your unique code\n2. Run \`/link <code>\` in-game with your code` }
            );

        const row = new ActionRowBuilder<ButtonBuilder>()
            .addComponents(
                new ButtonBuilder()
                    .setCustomId('link_button')
                    .setLabel('Link your account')
                    .setStyle(ButtonStyle.Primary)
            );

        await channel.send({ embeds: [embed], components: [row] });
        await interaction.reply({ content: `Link message sent to ${channel}!`, ephemeral: true });
    },

    async handleButton(interaction: ButtonInteraction) {
        const linkCode = generateSecureCode();
        const dbManager = new DatabaseManager();
        await dbManager.storeLinkCode(linkCode, interaction.user.id);

        await interaction.reply({ 
            content: `Run this command in-game:\n\`/link ${linkCode}\`\n\nThis code will expire in 5 minutes.`, 
            ephemeral: true 
        });
    }
};

export default command; 
