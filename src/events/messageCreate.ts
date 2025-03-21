import { Client, Message, MessageFlags, TextChannel } from 'discord.js';
import { deleteAdvertisingMessage } from '../utils/helpers';
import config from "../config/config";

export const onMessageCreate = async (client: Client, message: Message) => {
    console.log(message.channel.id)
    if (message.channel.id !== config.advertisingId) return;
    if (message.author.bot) return;

    const channel = message.channel as TextChannel;
    await deleteAdvertisingMessage(client);

    try {
        await channel.send({
            content: config.advertisingMessage,
            flags: [ MessageFlags.SuppressNotifications ] // @silent message
        });
    } catch (error) {
        console.error(`Could not send message in advertising channel (${config.advertisingId}): `, error);
    }
};
