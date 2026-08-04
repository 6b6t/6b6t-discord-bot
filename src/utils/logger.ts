import { type Client, EmbedBuilder } from "discord.js";

export interface BannerChangeLogOptions {
  guildId: string;
  submitterTag: string;
  submitterId: string;
  approverTag?: string;
  approverId?: string;
  imageUrl: string;
  adminBypass: boolean;
}

export async function logBannerChange(
  client: Client,
  opts: BannerChangeLogOptions,
): Promise<void> {
  const logChannelId = process.env.LOG_CHANNEL_ID;
  if (!logChannelId) {
    console.warn(
      "[Logger] LOG_CHANNEL_ID not set in .env — skipping audit log.",
    );
    return;
  }

  try {
    const channel = await client.channels.fetch(logChannelId);
    if (!channel?.isTextBased() || !("send" in channel)) {
      console.warn(
        `[Logger] Channel ${logChannelId} not found or not text-based.`,
      );
      return;
    }

    const embed = new EmbedBuilder()
      .setTitle("🖼️ Server Banner Changed")
      .setColor(0x2b2d31)
      .setImage(opts.imageUrl)
      .setTimestamp()
      .addFields({
        name: "Submitted By",
        value: `<@${opts.submitterId}> (${opts.submitterTag})`,
        inline: true,
      });

    if (opts.adminBypass) {
      embed.addFields({
        name: "Approved Via",
        value: "🛡️ Admin Bypass",
        inline: true,
      });
    } else if (opts.approverTag && opts.approverId) {
      embed.addFields({
        name: "Approved By",
        value: `<@${opts.approverId}> (${opts.approverTag})`,
        inline: true,
      });
    }

    embed.addFields({
      name: "Image URL",
      value: `[Click to view](${opts.imageUrl})`,
      inline: false,
    });

    await channel.send({ embeds: [embed] });
  } catch (error) {
    console.error("[Logger] Failed to send audit log:", error);
  }
}

export interface BanActionLogOptions {
  guildId: string;
  submitterTag: string;
  submitterId: string;
  approverTag?: string;
  approverId?: string;
  targetTag: string;
  targetId: string;
  reason: string;
  adminBypass: boolean;
}

export async function logBanAction(
  client: Client,
  opts: BanActionLogOptions,
): Promise<void> {
  const logChannelId = process.env.LOG_CHANNEL_ID;
  if (!logChannelId) {
    console.warn(
      "[Logger] LOG_CHANNEL_ID not set in .env — skipping audit log.",
    );
    return;
  }

  try {
    const channel = await client.channels.fetch(logChannelId);
    if (!channel?.isTextBased() || !("send" in channel)) {
      console.warn(
        `[Logger] Channel ${logChannelId} not found or not text-based.`,
      );
      return;
    }

    const embed = new EmbedBuilder()
      .setTitle("🔨 User Banned")
      .setColor(0xed4245)
      .setTimestamp()
      .addFields(
        {
          name: "Banned User",
          value: `<@${opts.targetId}> (${opts.targetTag})`,
          inline: true,
        },
        {
          name: "Banned By",
          value: `<@${opts.submitterId}> (${opts.submitterTag})`,
          inline: true,
        },
        { name: "Reason", value: opts.reason, inline: false },
      );

    if (opts.adminBypass) {
      embed.addFields({
        name: "Approved Via",
        value: "🛡️ Admin Bypass",
        inline: true,
      });
    } else if (opts.approverTag && opts.approverId) {
      embed.addFields({
        name: "Approved By",
        value: `<@${opts.approverId}> (${opts.approverTag})`,
        inline: true,
      });
    }

    await channel.send({ embeds: [embed] });
  } catch (error) {
    console.error("[Logger] Failed to send ban audit log:", error);
  }
}

export type MiniTerminatorAction = "add" | "remove";

export interface MiniTerminatorRoleChangeLogOptions {
  guildId: string;
  action: MiniTerminatorAction;
  submitterTag: string;
  submitterId: string;
  approverTag?: string;
  approverId?: string;
  targetTag: string;
  targetId: string;
  adminBypass: boolean;
}

export async function logMiniTerminatorRoleChange(
  client: Client,
  opts: MiniTerminatorRoleChangeLogOptions,
): Promise<void> {
  const logChannelId = process.env.LOG_CHANNEL_ID;
  if (!logChannelId) {
    console.warn(
      "[Logger] LOG_CHANNEL_ID not set in .env — skipping audit log.",
    );
    return;
  }

  try {
    const channel = await client.channels.fetch(logChannelId);
    if (!channel?.isTextBased() || !("send" in channel)) {
      console.warn(
        `[Logger] Channel ${logChannelId} not found or not text-based.`,
      );
      return;
    }

    const embed = new EmbedBuilder()
      .setTitle(
        opts.action === "add"
          ? "🤖 Mini-Terminator Role Granted"
          : "🤖 Mini-Terminator Role Removed",
      )
      .setColor(opts.action === "add" ? 0x57f287 : 0xed4245)
      .setTimestamp()
      .addFields(
        {
          name: "Target",
          value: `<@${opts.targetId}> (${opts.targetTag})`,
          inline: true,
        },
        {
          name: "Submitted By",
          value: `<@${opts.submitterId}> (${opts.submitterTag})`,
          inline: true,
        },
        {
          name: "Action",
          value: opts.action === "add" ? "➕ Role added" : "➖ Role removed",
          inline: false,
        },
      );

    if (opts.adminBypass) {
      embed.addFields({
        name: "Approved Via",
        value: "🛡️ Admin Bypass",
        inline: true,
      });
    } else if (opts.approverTag && opts.approverId) {
      embed.addFields({
        name: "Approved By",
        value: `<@${opts.approverId}> (${opts.approverTag})`,
        inline: true,
      });
    }

    await channel.send({ embeds: [embed] });
  } catch (error) {
    console.error("[Logger] Failed to send audit log:", error);
  }
}
