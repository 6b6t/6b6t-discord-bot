import { randomUUID } from "node:crypto";
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
import {
  getMediaChannelFrequency,
  getMediaChannelFrequencyBounds,
  setMediaChannelFrequency,
} from "../utils/mediaChannels";
import { CONFIRMER_ROLE_ID, isTerminator } from "../utils/roles";

type PendingFrequencyRequest = {
  id: string;
  submitterId: string;
  submitterTag: string;
  currentFrequency: number;
  newFrequency: number;
  createdAt: number;
};

const REQUEST_TTL_MS = 60 * 60 * 1000;
const pendingFrequencyRequests = new Map<string, PendingFrequencyRequest>();

function createFrequencyRequest(data: {
  submitterId: string;
  submitterTag: string;
  currentFrequency: number;
  newFrequency: number;
}): string {
  const id = randomUUID();
  pendingFrequencyRequests.set(id, {
    id,
    ...data,
    createdAt: Date.now(),
  });
  return id;
}

function getFrequencyRequest(id: string): PendingFrequencyRequest | null {
  const request = pendingFrequencyRequests.get(id);
  if (!request) return null;

  if (Date.now() - request.createdAt > REQUEST_TTL_MS) {
    pendingFrequencyRequests.delete(id);
    return null;
  }

  return request;
}

function removeFrequencyRequest(id: string): void {
  pendingFrequencyRequests.delete(id);
}

function cleanupExpiredFrequencyRequests(): void {
  const now = Date.now();
  for (const [id, request] of pendingFrequencyRequests) {
    if (now - request.createdAt > REQUEST_TTL_MS) {
      pendingFrequencyRequests.delete(id);
    }
  }
}

setInterval(cleanupExpiredFrequencyRequests, 10 * 60 * 1000).unref();

function buildDisabledRow(requestId: string) {
  return new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder()
      .setCustomId(`mediafreq_approve_${requestId}`)
      .setLabel("Approve")
      .setStyle(ButtonStyle.Success)
      .setDisabled(true),
    new ButtonBuilder()
      .setCustomId(`mediafreq_reject_${requestId}`)
      .setLabel("Reject")
      .setStyle(ButtonStyle.Danger)
      .setDisabled(true),
  );
}

const MediaChannelsFreqCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("mediachannelsfreq")
    .setDescription("Change media channel reminder frequency")
    .addIntegerOption((option) => {
      const { min, max } = getMediaChannelFrequencyBounds();
      return option
        .setName("number")
        .setDescription("Messages between reminders")
        .setRequired(true)
        .setMinValue(min)
        .setMaxValue(max);
    }),

  cooldown: 0,

  async execute(interaction: ChatInputCommandInteraction) {
    const member = interaction.member as GuildMember;

    if (!isTerminator(member)) {
      await interaction.reply({
        content:
          "❌ Only members with the **Terminator** role can change media channel frequency.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const newFrequency = interaction.options.getInteger("number", true);
    const currentFrequency = await getMediaChannelFrequency();

    if (newFrequency === currentFrequency) {
      await interaction.reply({
        content: `Media channel reminders are already set to every ${currentFrequency} message(s).`,
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const voteChannelId = process.env.VOTE_CHANNEL_ID;
    const voteChannel = voteChannelId
      ? await interaction.client.channels.fetch(voteChannelId).catch(() => null)
      : null;

    if (!voteChannel?.isTextBased() || !("send" in voteChannel)) {
      await interaction.reply({
        content:
          "❌ Vote channel is not configured or not found. Please set `VOTE_CHANNEL_ID` in `.env`.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const requestId = createFrequencyRequest({
      submitterId: member.id,
      submitterTag: member.user.tag,
      currentFrequency,
      newFrequency,
    });
    const expiresAt = Math.floor((Date.now() + REQUEST_TTL_MS) / 1000);

    const confirmEmbed = new EmbedBuilder()
      .setTitle("Media Channel Frequency Change")
      .setDescription(
        `${member} wants to change the media channel reminder frequency.\n\n` +
          "A **different Terminator** must approve this request.\n" +
          `Expires: <t:${expiresAt}:R>`,
      )
      .setColor(0xfee75c)
      .addFields(
        {
          name: "Current Frequency",
          value: `Every ${currentFrequency} message(s)`,
          inline: true,
        },
        {
          name: "New Frequency",
          value: `Every ${newFrequency} message(s)`,
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
        .setCustomId(`mediafreq_approve_${requestId}`)
        .setLabel("Approve")
        .setStyle(ButtonStyle.Success),
      new ButtonBuilder()
        .setCustomId(`mediafreq_reject_${requestId}`)
        .setLabel("Reject")
        .setStyle(ButtonStyle.Danger),
    );

    await voteChannel.send({
      content: `<@&${CONFIRMER_ROLE_ID}> a media channel frequency change needs approval.`,
      allowedMentions: {
        parse: [],
        roles: [CONFIRMER_ROLE_ID],
        users: [],
        repliedUser: false,
      },
      embeds: [confirmEmbed],
      components: [row],
    });

    await interaction.reply({
      content: `Your media channel frequency change request has been submitted for approval in ${voteChannel}.`,
      flags: MessageFlags.Ephemeral,
    });
  },

  async handleButton(interaction: ButtonInteraction) {
    const customId = interaction.customId;
    const isApproval = customId.startsWith("mediafreq_approve_");
    const requestId = customId
      .replace("mediafreq_approve_", "")
      .replace("mediafreq_reject_", "");

    const request = getFrequencyRequest(requestId);
    if (!request) {
      await interaction.reply({
        content:
          "⏰ This frequency change request has **expired** or has already been processed.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const clicker = interaction.member as GuildMember;
    if (!isTerminator(clicker)) {
      await interaction.reply({
        content:
          "❌ Only members with the **Terminator** role can approve or reject media channel frequency changes.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    if (!isApproval) {
      removeFrequencyRequest(requestId);

      const originalEmbed = interaction.message.embeds[0];
      if (!originalEmbed) {
        await interaction.reply({
          content: "❌ Could not process this request.",
          flags: MessageFlags.Ephemeral,
        });
        return;
      }

      const statusIndex = originalEmbed.fields.findIndex(
        (field) => field.name === "Status",
      );
      const rejectedEmbed = EmbedBuilder.from(originalEmbed)
        .setColor(0xed4245)
        .setTitle("Media Channel Frequency Change Rejected");

      if (statusIndex !== -1) {
        rejectedEmbed.spliceFields(statusIndex, 1, {
          name: "Status",
          value: `❌ Rejected by ${clicker}`,
          inline: true,
        });
      }

      await interaction.update({
        embeds: [rejectedEmbed],
        components: [buildDisabledRow(requestId)],
      });
      return;
    }

    if (clicker.id === request.submitterId) {
      await interaction.reply({
        content:
          "⚠️ You **cannot approve your own** frequency change. A **different** Terminator must approve it.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const originalEmbed = interaction.message.embeds[0];
    if (!originalEmbed) {
      await interaction.reply({
        content: "❌ Could not process this request.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    await interaction.deferUpdate();

    await setMediaChannelFrequency(request.newFrequency);
    removeFrequencyRequest(requestId);

    const statusIndex = originalEmbed.fields.findIndex(
      (field) => field.name === "Status",
    );
    const approvedEmbed = EmbedBuilder.from(originalEmbed)
      .setColor(0x57f287)
      .setTitle("Media Channel Frequency Change Approved");

    if (statusIndex !== -1) {
      approvedEmbed.spliceFields(statusIndex, 1, {
        name: "Status",
        value: `✅ Approved by ${clicker}`,
        inline: true,
      });
    }

    await interaction.editReply({
      embeds: [approvedEmbed],
      components: [buildDisabledRow(requestId)],
    });
  },
};

export default MediaChannelsFreqCommand;
