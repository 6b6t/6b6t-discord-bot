import { SlashCommandBuilder } from '@discordjs/builders';
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
import { Command } from '../types/command';
import config from '../config/config';

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
            .setDescription('Click the button below and enter the code you received in-game to link your accounts.\n\nLinking will automatically give you all ranks that you purchased from [6b6t Shop](https://6b6t.org/shop).')
            .addFields(
                { name: 'How to link', value: '1. Run `/link` command in-game\n2. Click the button below\n3. Enter the code you received' }
            );

        const row = new ActionRowBuilder<ButtonBuilder>()
            .addComponents(
                new ButtonBuilder()
                    .setCustomId('link_button')
                    .setLabel('Enter Link Code')
                    .setStyle(ButtonStyle.Primary)
            );

        await channel.send({ embeds: [embed], components: [row] });
        await interaction.reply({ content: `Link message sent to ${channel}!`, ephemeral: true });
    },

    async handleButton(interaction: ButtonInteraction) {
        const modal = new ModalBuilder()
            .setCustomId('link_modal')
            .setTitle('Enter Link Code');

        const codeInput = new TextInputBuilder()
            .setCustomId('link_code')
            .setLabel('Enter the code you received in-game')
            .setStyle(TextInputStyle.Short)
            .setMinLength(1)
            .setMaxLength(4)
            .setRequired(true);

        const actionRow = new ActionRowBuilder<TextInputBuilder>().addComponents(codeInput);
        modal.addComponents(actionRow);

        await interaction.showModal(modal);
    },

    async handleModal(interaction: ModalSubmitInteraction, redisManager: RedisManager) {
        const code = interaction.fields.getTextInputValue('link_code');
        const success = await redisManager.verifyLinkCode(code, interaction.user.id);

        if (success) {
            await interaction.reply({ 
                content: 'Successfully linked your Minecraft account!', 
                ephemeral: true 
            });
        } else {
            await interaction.reply({ 
                content: 'Invalid or expired code. Please try again.', 
                ephemeral: true 
            });
        }
    }
};

export default command; 