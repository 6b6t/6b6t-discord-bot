import {
  ActionRowBuilder,
  ButtonBuilder,
  type ButtonInteraction,
  ButtonStyle,
  type ChatInputCommandInteraction,
  EmbedBuilder,
  type GuildMember,
  GuildPremiumTier,
  MessageFlags,
  SlashCommandBuilder,
} from "discord.js";
import type { Command } from "../types/command";
import { validateBannerImage } from "../utils/imageValidator";
import { logBannerChange } from "../utils/logger";
import {
  createRequest,
  getRequest,
  removeRequest,
  setMessageId,
  TTL_MS,
} from "../utils/pendingBanners";
import { hasAuthorizedRole, isAdmin, isTerminator } from "../utils/roles";

const DiscordBannerSetCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("discordbannerset")
    .setDescription(
      "Set the server banner (requires second Terminator approval)",
    )
    .addAttachmentOption((option) =>
      option
        .setName("image")
        .setDescription(
          "Upload a banner image (PNG, JPG, GIF, WebP — max 10 MB)",
        )
        .setRequired(false),
    )
    .addStringOption((option) =>
      option
        .setName("url")
        .setDescription("URL to a hosted banner image")
        .setRequired(false),
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

    const attachment = interaction.options.getAttachment("image");
    const url = interaction.options.getString("url");

    const validation = validateBannerImage(attachment, url);
    if (!validation.valid) {
      await interaction.reply({
        content: `❌ ${validation.error}`,
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const { imageUrl, isAnimated } = validation;

    const guild = interaction.guild!;
    const requiredTier = isAnimated
      ? GuildPremiumTier.Tier3
      : GuildPremiumTier.Tier2;
    const requiredLabel = isAnimated ? "Boost Level 3" : "Boost Level 2";

    if (guild.premiumTier < requiredTier) {
      await interaction.reply({
        content: `❌ This server needs **${requiredLabel}** to set ${isAnimated ? "an animated " : "a "}banner. Current tier: **Level ${guild.premiumTier}**.`,
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    if (isAdmin(member)) {
      await interaction.deferReply();

      try {
        await guild.setBanner(imageUrl!);

        const successEmbed = new EmbedBuilder()
          .setTitle("✅ Server Banner Updated")
          .setDescription(
            `Banner set by ${member} via **admin bypass** (no confirmation needed).`,
          )
          .setImage(imageUrl!)
          .setColor(0x57f287)
          .setTimestamp();

        await interaction.editReply({ embeds: [successEmbed] });

        await logBannerChange(interaction.client, {
          guildId: guild.id,
          submitterTag: member.user.tag,
          submitterId: member.id,
          imageUrl: imageUrl!,
          adminBypass: true,
        });
      } catch (error) {
        console.error("[BannerSet] Admin bypass failed:", error);
        await interaction.editReply({
          content: `❌ Failed to set the banner. Error: \`${(error as Error).message}\``,
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

    const requestId = createRequest({
      submitterId: member.id,
      submitterTag: member.user.tag,
      imageUrl: imageUrl!,
      guildId: guild.id,
      channelId: voteChannelId!,
    });

    const expiresAt = Math.floor((Date.now() + TTL_MS) / 1000);

    const confirmEmbed = new EmbedBuilder()
      .setTitle("🖼️ Banner Change Request")
      .setDescription(
        `${member} wants to change the server banner.\n\n` +
          `A **different Terminator** must approve this request.\n` +
          `Expires: <t:${expiresAt}:R>`,
      )
      .setImage(imageUrl!)
      .setColor(0xfee75c)
      .addFields(
        {
          name: "Submitted By",
          value: `${member} (${member.user.tag})`,
          inline: true,
        },
        {
          name: "Status",
          value: "⏳ Awaiting confirmation",
          inline: true,
        },
      )
      .setFooter({ text: `Request ID: ${requestId}` })
      .setTimestamp();

    const row = new ActionRowBuilder<ButtonBuilder>().addComponents(
      new ButtonBuilder()
        .setCustomId(`banner_approve_${requestId}`)
        .setLabel("Approve")
        .setStyle(ButtonStyle.Success)
        .setEmoji("✅"),
      new ButtonBuilder()
        .setCustomId(`banner_reject_${requestId}`)
        .setLabel("Reject")
        .setStyle(ButtonStyle.Danger)
        .setEmoji("❌"),
    );

    const voteMessage = await voteChannel.send({
      embeds: [confirmEmbed],
      components: [row],
    });

    setMessageId(requestId, voteMessage.id);

    await interaction.reply({
      content: `🖼️ Your banner change request has been submitted for approval in ${voteChannel}.`,
      flags: MessageFlags.Ephemeral,
    });
  },

  async handleButton(interaction: ButtonInteraction) {
    const customId = interaction.customId;
    const isApproval = customId.startsWith("banner_approve_");
    const requestId = customId
      .replace("banner_approve_", "")
      .replace("banner_reject_", "");

    const request = getRequest(requestId);

    if (!request) {
      await interaction.reply({
        content:
          "⏰ This banner request has **expired** or has already been processed.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const clicker = interaction.member as GuildMember;

    if (!isTerminator(clicker)) {
      await interaction.reply({
        content:
          "❌ Only members with the **Terminator** role can approve or reject banner requests.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    if (!isApproval) {
      removeRequest(requestId);

      const rejectedEmbed = EmbedBuilder.from(interaction.message.embeds[0])
        .setColor(0xed4245)
        .setTitle("❌ Banner Change Rejected")
        .spliceFields(1, 1, {
          name: "Status",
          value: `❌ Rejected by ${clicker}`,
          inline: true,
        });

      const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder()
          .setCustomId(`banner_approve_${requestId}`)
          .setLabel("Approve")
          .setStyle(ButtonStyle.Success)
          .setEmoji("✅")
          .setDisabled(true),
        new ButtonBuilder()
          .setCustomId(`banner_reject_${requestId}`)
          .setLabel("Reject")
          .setStyle(ButtonStyle.Danger)
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
          "⚠️ You **cannot approve your own** banner submission. A **different** Terminator must approve it.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    await interaction.deferUpdate();

    try {
      const guild = interaction.guild!;
      await guild.setBanner(request.imageUrl);

      removeRequest(requestId);

      const approvedEmbed = EmbedBuilder.from(interaction.message.embeds[0])
        .setColor(0x57f287)
        .setTitle("✅ Banner Change Approved")
        .spliceFields(1, 1, {
          name: "Status",
          value: `✅ Approved by ${clicker}`,
          inline: true,
        });

      const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder()
          .setCustomId(`banner_approve_${requestId}`)
          .setLabel("Approve")
          .setStyle(ButtonStyle.Success)
          .setEmoji("✅")
          .setDisabled(true),
        new ButtonBuilder()
          .setCustomId(`banner_reject_${requestId}`)
          .setLabel("Reject")
          .setStyle(ButtonStyle.Danger)
          .setEmoji("❌")
          .setDisabled(true),
      );

      await interaction.editReply({
        embeds: [approvedEmbed],
        components: [disabledRow],
      });

      await logBannerChange(interaction.client, {
        guildId: guild.id,
        submitterTag: request.submitterTag,
        submitterId: request.submitterId,
        approverTag: clicker.user.tag,
        approverId: clicker.id,
        imageUrl: request.imageUrl,
        adminBypass: false,
      });
    } catch (error) {
      console.error("[BannerSet] Approval failed:", error);
      await interaction.editReply({
        content: `❌ Failed to set the banner. Error: \`${(error as Error).message}\``,
        embeds: [],
        components: [],
      });
    }
  },
};

export default DiscordBannerSetCommand;
