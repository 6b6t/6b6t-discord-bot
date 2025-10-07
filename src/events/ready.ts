import {
  ChannelType,
  Client,
  ActivityType,
  EmbedBuilder,
  Colors,
} from 'discord.js';
import { sync } from './sync';
import { botHasRecentMessages, getServerData } from '../utils/helpers';
import { sendYoutubeNotification } from '../utils/youtube';
import cron from 'cron';
import config from '../config/config';
import { existsRoleMenu, sendRoleMenu } from '../utils/menu';
import { sendReactionRoleMenu } from '../utils/reactionMenu';

export const onReady = async (client: Client) => {
  console.log(`Logged in as ${client.user?.tag}!`);

  async function runSync() {
    console.log('Running sync...');
    try {
      if (client.isReady()) {
        await sync(client);
      }
    } finally {
      setTimeout(runSync, 30_000);
    }
  }

  async function sendReminder() {
    const channel = await client.channels.fetch(config.generalId);
    if (channel && channel.type === ChannelType.GuildText) {
      await channel.send(config.generalMessage);
    } else {
      console.error(
        `Couldn't find general channel by ID: ${config.generalId} ${channel}`,
      );
    }
  }

  async function sendNotification() {
    const youtubeChannel = await client.channels.fetch(config.youtubeId);
    if (!youtubeChannel) {
      console.error(
        `Couldn't find youtube videos channel by ID: ${config.youtubeId} ${youtubeChannel}`,
      );
      return;
    }

    if (youtubeChannel.type !== ChannelType.GuildAnnouncement) {
      console.error(
        `Youtube videos channel (${config.youtubeId} ${youtubeChannel}) isn't an announcement channel`,
      );
      return;
    }

    await sendYoutubeNotification(
      youtubeChannel,
      config.youtubeQueries,
      config.youtubeIgnoreWords,
      config.youtubeWhitelistedIds,
    );
  }

  async function sendRoleMenuMsg() {
    const roleChannel = await client.channels.fetch(config.roleMenuId);
    if (!roleChannel) {
      console.error(
        `Couldn't find role menu channel by ID: ${config.roleMenuId} ${roleChannel}`,
      );
      return;
    }

    if (roleChannel.type !== ChannelType.GuildText) {
      console.error(
        `Role menu channel (${config.roleMenuId} ${roleChannel}) isn't a text channel`,
      );
      return;
    }

    const existsMenu = await existsRoleMenu(roleChannel);
    if (existsMenu) return;

    await sendRoleMenu(roleChannel);
  }

  async function sendReactionRoleMenus() {
    const reactionRoleChannel = await client.channels.fetch(
      config.reactionRoleMenuId,
    );
    if (!reactionRoleChannel) {
      console.error(
        `Couldn't find reaction role channel by ID: ${config.reactionRoleMenuId} ${reactionRoleChannel}`,
      );
      return;
    }

    if (reactionRoleChannel.type !== ChannelType.GuildText) {
      console.error(
        `Reaction role channel (${config.reactionRoleMenuId} ${reactionRoleChannel}) isn't a text channel`,
      );
      return;
    }

    const hasMessages = await botHasRecentMessages(reactionRoleChannel, client);
    if (hasMessages) return;

    const languageEmbed = new EmbedBuilder()
      .setAuthor({
        name: '6b6t.org',
        iconURL: 'https://www.6b6t.org/logo.png',
      })
      .setDescription(
        `
Select your language.
      `,
      )
      .setColor('#07CFFA');

    const notificationEmbed = new EmbedBuilder()
      .setAuthor({
        name: '6b6t.org',
        iconURL: 'https://www.6b6t.org/logo.png',
      })
      .setImage('https://www.6b6t.org/media/language-and-roles.gif')
      .setDescription(
        `
Select your notifications.

✨ - General changes to 6b6t
⚔️ - Crystal PvP, anticheat changes, PvP events and more
🌩️ - Server going offline, online or restarting
🎉 - Events and competitions in Discord and Minecraft
🏄 - Help us test new features
🎥 - Receive social media notifications
      `,
      )
      .setColor('#FFF11A');

    await sendReactionRoleMenu(
      reactionRoleChannel,
      config.languageMenuRoleIds,
      languageEmbed,
    );
    await sendReactionRoleMenu(
      reactionRoleChannel,
      config.notificationMenuRoleIds,
      notificationEmbed,
    );
  }

  async function updateStatus() {
    if (!client.user) return;
    const data = await getServerData(config.statusHost);
    if (!data) return;
    client.user.setActivity(
      `IP: ${config.statusHost} - Join ${data.players.now} other players online!`,
      { type: ActivityType.Playing },
    );
  }

  await updateStatus();
  new cron.CronJob('*/5 * * * *', updateStatus, null, true, 'Europe/Berlin');

  new cron.CronJob('0 10 * * *', sendReminder, null, true, 'Europe/Berlin');

  new cron.CronJob('0 18 * * *', sendReminder, null, true, 'Europe/Berlin');

  /*
  Free trial is 100 checks a day, which is 1 check every 15 minutes, 20 minutes to be safe
  This would ignore any video sent before those 20 minutes, but it can't be fixed without paying or using IFTTT
  */

  // Run once at startup for debugging
  void sendNotification();
  new cron.CronJob(
    '*/20 * * * *',
    sendNotification,
    null,
    true,
    'Europe/Berlin',
  );

  void sendRoleMenuMsg();
  void sendReactionRoleMenus();
  void runSync();
};
