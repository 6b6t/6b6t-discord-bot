import {
  ActionRowBuilder,
  ButtonBuilder,
  type ButtonInteraction,
  ButtonStyle,
  type ChatInputCommandInteraction,
  EmbedBuilder,
  type GuildMember,
  MessageFlags,
  SlashCommandBuilder,
} from "discord.js";
import type { Command } from "../types/command";
import { logBanAction } from "../utils/logger";
import {
  BAN_TTL_MS,
  createBanRequest,
  getBanRequest,
  removeBanRequest,
  setBanMessageId,
} from "../utils/pendingBans";
import { hasAuthorizedRole, isAdmin, isTerminator } from "../utils/roles";

const TerminatorBanCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("terminatorban")
    .setDescription("Ban a user (requires second Terminator approval)")
    .addUserOption((option) =>
      option
        .setName("user")
        .setDescription("The user to ban")
        .setRequired(true),
    )
    .addStringOption((option) =>
      option
        .setName("reason")
        .setDescription("Reason for the ban")
        .setRequired(false),
    )
    .addIntegerOption((option) =>
      option
        .setName("delete_messages")
        .setDescription("Days of messages to delete (0-7)")
        .setRequired(false)
        .setMinValue(0)
        .setMaxValue(7),
    ),

  cooldown: 0,

  async execute(interaction: ChatInputCommandInteraction) {
    const member = interaction.member as GuildMember;

    if (!isAdmin(member) && !hasAuthorizedRole(member)) {
      await interaction.reply({
        content:
          "❌ You do not have permission to use this command. Required roles: **Terminator**, **Marketer**, **Dev**.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const targetUser = interaction.options.getUser("user", true);
    const reason =
      interaction.options.getString("reason") ?? "No reason provided";
    const deleteMessageDays =
      interaction.options.getInteger("delete_messages") ?? 0;

    const guild = interaction.guild!;

    if (targetUser.id === member.id) {
      await interaction.reply({
        content: "❌ You cannot ban yourself.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    if (targetUser.id === interaction.client.user?.id) {
      await interaction.reply({
        content: "❌ I cannot ban myself.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    let targetMember: GuildMember | null = null;
    try {
      targetMember = await guild.members.fetch(targetUser.id);
    } catch {}

    if (targetMember) {
      if (
        targetMember.roles.highest.position >= member.roles.highest.position &&
        !isAdmin(member)
      ) {
        await interaction.reply({
          content:
            "❌ You cannot ban someone with a **higher or equal** role than yours.",
          flags: MessageFlags.Ephemeral,
        });
        return;
      }

      const botMember = guild.members.me!;
      if (
        targetMember.roles.highest.position >= botMember.roles.highest.position
      ) {
        await interaction.reply({
          content:
            "❌ I cannot ban this user — their role is **higher or equal** to mine.",
          flags: MessageFlags.Ephemeral,
        });
        return;
      }

      if (!targetMember.bannable) {
        await interaction.reply({
          content:
            "❌ I cannot ban this user. They may be the server owner or have special protections.",
          flags: MessageFlags.Ephemeral,
        });
        return;
      }
    }

    if (isAdmin(member)) {
      await interaction.deferReply();

      try {
        await guild.members.ban(targetUser.id, {
          deleteMessageSeconds: deleteMessageDays * 86400,
          reason: `Banned by ${member.user.tag} (admin bypass) — ${reason}`,
        });

        const successEmbed = new EmbedBuilder()
          .setTitle("🔨 User Banned")
          .setDescription(
            `${targetUser} has been banned by ${member} via **admin bypass**.\n` +
              "No second confirmation was needed.",
          )
          .setColor(0xed4245)
          .addFields(
            {
              name: "Banned User",
              value: `${targetUser.tag} (${targetUser.id})`,
              inline: true,
            },
            { name: "Reason", value: reason, inline: true },
            {
              name: "Messages Deleted",
              value: `${deleteMessageDays} day(s)`,
              inline: true,
            },
          )
          .setThumbnail(targetUser.displayAvatarURL())
          .setTimestamp();

        await interaction.editReply({ embeds: [successEmbed] });

        await logBanAction(interaction.client, {
          guildId: guild.id,
          submitterTag: member.user.tag,
          submitterId: member.id,
          targetTag: targetUser.tag,
          targetId: targetUser.id,
          reason,
          adminBypass: true,
        });
      } catch (error) {
        console.error("[TerminatorBan] Admin bypass failed:", error);
        await interaction.editReply({
          content: `❌ Failed to ban user. Error: \`${(error as Error).message}\``,
        });
      }

      return;
    }

    const voteChannelId = process.env.VOTE_CHANNEL_ID;
    const voteChannel = voteChannelId
      ? await interaction.client.channels.fetch(voteChannelId).catch(() => null)
      : null;

    if (
      !voteChannel ||
      !voteChannel.isTextBased() ||
      !("send" in voteChannel)
    ) {
      await interaction.reply({
        content:
          "❌ Vote channel is not configured or not found. Please set `VOTE_CHANNEL_ID` in `.env`.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const requestId = createBanRequest({
      submitterId: member.id,
      submitterTag: member.user.tag,
      targetId: targetUser.id,
      targetTag: targetUser.tag,
      reason,
      deleteMessageDays,
      guildId: guild.id,
      channelId: voteChannelId!,
    });

    const expiresAt = Math.floor((Date.now() + BAN_TTL_MS) / 1000);

    const confirmEmbed = new EmbedBuilder()
      .setTitle("🔨 Ban Request")
      .setDescription(
        `${member} wants to ban ${targetUser}.\n\n` +
          `A **different Terminator** must approve this request.\n` +
          `Expires: <t:${expiresAt}:R>`,
      )
      .setColor(0xfee75c)
      .addFields(
        {
          name: "Target",
          value: `${targetUser.tag} (${targetUser.id})`,
          inline: true,
        },
        {
          name: "Requested By",
          value: `${member} (${member.user.tag})`,
          inline: true,
        },
        { name: "Reason", value: reason, inline: false },
        {
          name: "Messages to Delete",
          value: `${deleteMessageDays} day(s)`,
          inline: true,
        },
        {
          name: "Status",
          value: "⏳ Awaiting confirmation",
          inline: true,
        },
      )
      .setThumbnail(targetUser.displayAvatarURL())
      .setFooter({ text: `Request ID: ${requestId}` })
      .setTimestamp();

    const row = new ActionRowBuilder<ButtonBuilder>().addComponents(
      new ButtonBuilder()
        .setCustomId(`ban_approve_${requestId}`)
        .setLabel("Approve Ban")
        .setStyle(ButtonStyle.Danger)
        .setEmoji("🔨"),
      new ButtonBuilder()
        .setCustomId(`ban_reject_${requestId}`)
        .setLabel("Reject")
        .setStyle(ButtonStyle.Secondary)
        .setEmoji("❌"),
    );

    const voteMessage = await voteChannel.send({
      embeds: [confirmEmbed],
      components: [row],
    });

    setBanMessageId(requestId, voteMessage.id);

    await interaction.reply({
      content: `🔨 Your ban request for **${targetUser.tag}** has been submitted for approval in ${voteChannel}.`,
      flags: MessageFlags.Ephemeral,
    });
  },

  async handleButton(interaction: ButtonInteraction) {
    const customId = interaction.customId;
    const isApproval = customId.startsWith("ban_approve_");
    const requestId = customId
      .replace("ban_approve_", "")
      .replace("ban_reject_", "");

    const request = getBanRequest(requestId);

    if (!request) {
      await interaction.reply({
        content:
          "⏰ This ban request has **expired** or has already been processed.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const clicker = interaction.member as GuildMember;

    if (!isTerminator(clicker)) {
      await interaction.reply({
        content:
          "❌ Only members with the **Terminator** role can approve or reject ban requests.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    if (!isApproval) {
      removeBanRequest(requestId);

      const embedFields = interaction.message.embeds[0].fields;
      const statusIndex = embedFields.findIndex((f) => f.name === "Status");

      const rejectedEmbed = EmbedBuilder.from(interaction.message.embeds[0])
        .setColor(0x95a5a6)
        .setTitle("❌ Ban Request Rejected");

      if (statusIndex !== -1) {
        rejectedEmbed.spliceFields(statusIndex, 1, {
          name: "Status",
          value: `❌ Rejected by ${clicker}`,
          inline: true,
        });
      }

      const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder()
          .setCustomId(`ban_approve_${requestId}`)
          .setLabel("Approve Ban")
          .setStyle(ButtonStyle.Danger)
          .setEmoji("🔨")
          .setDisabled(true),
        new ButtonBuilder()
          .setCustomId(`ban_reject_${requestId}`)
          .setLabel("Reject")
          .setStyle(ButtonStyle.Secondary)
          .setEmoji("❌")
          .setDisabled(true),
      );

      await interaction.update({
        embeds: [rejectedEmbed],
        components: [disabledRow],
      });
      return;
    }

    if (clicker.id === request.submitterId) {
      await interaction.reply({
        content:
          "⚠️ You **cannot approve your own** ban request. A **different** Terminator must approve it.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    await interaction.deferUpdate();

    try {
      const guild = interaction.guild!;
      await guild.members.ban(request.targetId, {
        deleteMessageSeconds: request.deleteMessageDays * 86400,
        reason: `Banned by ${request.submitterTag}, approved by ${clicker.user.tag} — ${request.reason}`,
      });

      removeBanRequest(requestId);

      const embedFields = interaction.message.embeds[0].fields;
      const statusIndex = embedFields.findIndex((f) => f.name === "Status");

      const approvedEmbed = EmbedBuilder.from(interaction.message.embeds[0])
        .setColor(0xed4245)
        .setTitle("🔨 Ban Approved & Executed");

      if (statusIndex !== -1) {
        approvedEmbed.spliceFields(statusIndex, 1, {
          name: "Status",
          value: `✅ Approved by ${clicker}`,
          inline: true,
        });
      }

      const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder()
          .setCustomId(`ban_approve_${requestId}`)
          .setLabel("Approve Ban")
          .setStyle(ButtonStyle.Danger)
          .setEmoji("🔨")
          .setDisabled(true),
        new ButtonBuilder()
          .setCustomId(`ban_reject_${requestId}`)
          .setLabel("Reject")
          .setStyle(ButtonStyle.Secondary)
          .setEmoji("❌")
          .setDisabled(true),
      );

      await interaction.editReply({
        embeds: [approvedEmbed],
        components: [disabledRow],
      });

      await logBanAction(interaction.client, {
        guildId: guild.id,
        submitterTag: request.submitterTag,
        submitterId: request.submitterId,
        approverTag: clicker.user.tag,
        approverId: clicker.id,
        targetTag: request.targetTag,
        targetId: request.targetId,
        reason: request.reason,
        adminBypass: false,
      });
    } catch (error) {
      console.error("[TerminatorBan] Approval failed:", error);
      await interaction.editReply({
        content: `❌ Failed to ban user. Error: \`${(error as Error).message}\``,
        embeds: [],
        components: [],
      });
    }
  },
};

export default TerminatorBanCommand;
