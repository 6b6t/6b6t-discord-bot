import { ChannelType, Client, Message, MessageFlags } from 'discord.js';
import { deleteAdvertisingMessage } from '../utils/helpers';
import config from '../config/config';

export const onMessageCreate = async (client: Client, message: Message) => {
  if (!(message.channel.type === ChannelType.GuildText)) return;
  if (message.channel.id !== config.advertisingId) return;
  if (message.author.bot) return;

  const channel = message.channel;
  await deleteAdvertisingMessage(client, channel);

  try {
    await channel.send({
      content: config.advertisingMessage,
      flags: [MessageFlags.SuppressNotifications],
    });
  } catch (error) {
    console.error(
      `Could not send message in advertising channel (${config.advertisingId}): `,
      error,
    );
  }
};
