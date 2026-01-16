import cron from "cron";
import {
  ActivityType,
  ChannelType,
  type Client,
  EmbedBuilder,
  PermissionsBitField,
} from "discord.js";
import config from "../config/config";
import { botHasRecentMessages, getServerData } from "../utils/helpers";
import { existsRoleMenu, sendRoleMenu } from "../utils/menu";
import { sendReactionRoleMenu } from "../utils/reactionMenu";
import { sendYoutubeNotification } from "../utils/youtube";
import { sync } from "./sync";

export const onReady = async (client: Client) => {
  console.log("[Ready] Bot initialization starting...");
  console.log(
    `[Ready] Logged in as ${client.user?.tag} (ID: ${client.user?.id})`,
  );
  console.log(`[Ready] Connected to ${client.guilds.cache.size} guild(s)`);

  const cronLog = (job: string, message: string) =>
    console.log(`[Cron][${job}] ${message}`);

  async function runSync() {
    console.log("Running sync...");
    try {
      if (client.isReady()) {
        await sync(client);
      }
    } finally {
      setTimeout(runSync, 30_000);
    }
  }

  async function sendReminder() {
    cronLog("SendReminder", "Fetching channel");
    const channel = await client.channels.fetch(config.generalId);
    if (channel && channel.type === ChannelType.GuildText) {
      cronLog("SendReminder", `Sending reminder to channel ${channel.id}`);
      await channel.send(config.generalMessage);
    } else {
      console.error(
        `Couldn't find general channel by ID: ${config.generalId} ${channel}`,
      );
    }
    cronLog("SendReminder", "Finished");
  }

  async function sendNotification() {
    cronLog("SendNotification", "Fetching YouTube channel");
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

    cronLog("SendNotification", "Sending YouTube notification");
    await sendYoutubeNotification(
      youtubeChannel,
      config.youtubeQueries,
      config.youtubeIgnoreWords,
      config.youtubeWhitelistedIds,
    );
    cronLog("SendNotification", "Finished");
  }

  async function sendRoleMenuMsg() {
    cronLog("SendRoleMenuMsg", "Fetching role menu channel");
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

    cronLog("SendRoleMenuMsg", "Checking if role menu already exists");
    const existsMenu = await existsRoleMenu(roleChannel);
    if (existsMenu) {
      cronLog("SendRoleMenuMsg", "Role menu already exists, skipping");
      return;
    }

    cronLog("SendRoleMenuMsg", "Sending role menu");
    await sendRoleMenu(roleChannel);
    cronLog("SendRoleMenuMsg", "Finished");
  }

  async function cleanRoleMenuRoles() {
    cronLog("CleanRoleMenuRoles", "Fetching guild");
    const guild = await client.guilds.fetch(config.guildId);
    if (!guild) {
      cronLog("CleanRoleMenuRoles", "Guild not found, skipping");
      return;
    }

    try {
      await guild.members.fetch();
    } catch (error) {
      console.error(
        "[Cron][CleanRoleMenuRoles] Failed to fetch guild members:",
        error,
      );
      return;
    }

    const memberIdsWithMenuRoles = new Set<string>();

    for (const roleId of config.roleMenuRoleIds) {
      try {
        const role =
          guild.roles.cache.get(roleId) ?? (await guild.roles.fetch(roleId));
        if (!role) {
          cronLog("CleanRoleMenuRoles", `Role ${roleId} not found, skipping`);
          continue;
        }

        role.members.forEach((member) => {
          memberIdsWithMenuRoles.add(member.id);
        });
      } catch (error) {
        console.error(
          `[Cron][CleanRoleMenuRoles] Failed to inspect role ${roleId}:`,
          error,
        );
      }
    }

    let removedCount = 0;
    for (const memberId of memberIdsWithMenuRoles) {
      const member = guild.members.cache.get(memberId);
      if (!member) continue;

      const hasAccess =
        member.roles.cache.has(config.roleMenuRequiredRoleId) ||
        member.permissions.has(PermissionsBitField.Flags.Administrator);

      if (hasAccess) continue;

      try {
        await member.roles.remove(config.roleMenuRoleIds);
        removedCount += 1;
      } catch (error) {
        console.error(
          `[Cron][CleanRoleMenuRoles] Failed to remove role menu roles from ${member.id}:`,
          error,
        );
      }
    }

    cronLog(
      "CleanRoleMenuRoles",
      `Removed role menu roles from ${removedCount} member(s)`,
    );
  }

  async function sendReactionRoleMenus() {
    cronLog("SendReactionRoleMenus", "Fetching reaction role channel");
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

    cronLog("SendReactionRoleMenus", "Building embed messages");
    const embeds = [
      new EmbedBuilder()
        .setAuthor({
          name: "6b6t.org",
          iconURL: "https://www.6b6t.org/logo.png",
        })
        .setDescription(
          `
Select your language.
      `,
        )
        .setColor("#07CFFA"),
      new EmbedBuilder()
        .setAuthor({
          name: "6b6t.org",
          iconURL: "https://www.6b6t.org/logo.png",
        })
        .setImage("https://www.6b6t.org/media/language-and-roles.gif")
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
        .setColor("#FFF11A"),
      new EmbedBuilder()
        .setAuthor({
          name: "6b6t.org",
          iconURL: "https://www.6b6t.org/logo.png",
        })
        .setDescription(
          `
🎮 - Get notifications about Hytale.
      `,
        )
        .setColor("#82c0ef"),
    ];

    cronLog("SendReactionRoleMenus", "Checking for existing bot messages");
    const messageCount = await botHasRecentMessages(
      reactionRoleChannel,
      client,
    );
    if (messageCount < embeds.length) {
      cronLog(
        "SendReactionRoleMenus",
        `Found ${messageCount} existing messages (need ${embeds.length}), skipping`,
      );
      return;
    }

    cronLog(
      "SendReactionRoleMenus",
      `Sending ${embeds.length} reaction role menu(s)`,
    );
    for (const embed of embeds) {
      await sendReactionRoleMenu(
        reactionRoleChannel,
        config.languageMenuRoleIds,
        embed,
      );
    }
    cronLog("SendReactionRoleMenus", "Finished");
  }

  async function updateStatus() {
    if (!client.user) {
      cronLog("UpdateStatus", "Client user unavailable, skipping");
      return;
    }
    cronLog("UpdateStatus", "Fetching server data");
    const data = await getServerData();
    if (!data) {
      cronLog("UpdateStatus", "No data received from server");
      return;
    }
    cronLog(
      "UpdateStatus",
      `Setting activity with ${data.playerCount} players online`,
    );
    client.user.setActivity(
      `IP: ${config.statusHost} - Join ${data.playerCount} other players online!`,
      { type: ActivityType.Playing },
    );
    cronLog("UpdateStatus", "Finished");
  }

  console.log("[Ready] Running initial status update...");
  await updateStatus();
  console.log("[Ready] Initial status update complete");

  console.log("[Ready] Scheduling cron jobs...");
  new cron.CronJob("*/5 * * * *", updateStatus, null, true, "Europe/Berlin");
  console.log("[Ready] Scheduled UpdateStatus cron (every 5 minutes)");

  new cron.CronJob(
    "*/5 * * * *",
    cleanRoleMenuRoles,
    null,
    true,
    "Europe/Berlin",
  );
  console.log("[Ready] Scheduled CleanRoleMenuRoles cron (every 5 minutes)");

  new cron.CronJob("0 10 * * *", sendReminder, null, true, "Europe/Berlin");
  console.log("[Ready] Scheduled SendReminder cron (daily at 10:00)");

  new cron.CronJob("0 18 * * *", sendReminder, null, true, "Europe/Berlin");
  console.log("[Ready] Scheduled SendReminder cron (daily at 18:00)");

  /*
  Free trial is 100 checks a day, which is 1 check every 15 minutes, 20 minutes to be safe
  This would ignore any video sent before those 20 minutes, but it can't be fixed without paying or using IFTTT
  */

  new cron.CronJob(
    "*/20 * * * *",
    sendNotification,
    null,
    true,
    "Europe/Berlin",
  );
  console.log("[Ready] Scheduled SendNotification cron (every 20 minutes)");

  console.log("[Ready] Starting initial background tasks...");
  console.log("[Ready] Starting SendNotification...");
  void sendNotification();
  console.log("[Ready] Starting SendRoleMenuMsg...");
  void sendRoleMenuMsg();
  console.log("[Ready] Starting CleanRoleMenuRoles...");
  void cleanRoleMenuRoles();
  console.log("[Ready] Starting SendReactionRoleMenus...");
  void sendReactionRoleMenus();
  console.log("[Ready] Starting RunSync...");
  void runSync();

  console.log("[Ready] Bot initialization complete - all tasks started");
};
