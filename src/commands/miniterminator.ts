import {
  ActionRowBuilder,
  ButtonBuilder,
  type ButtonInteraction,
  ButtonStyle,
  type ChatInputCommandInteraction,
  EmbedBuilder,
  type Guild,
  type GuildMember,
  MessageFlags,
  SlashCommandBuilder,
} from "discord.js";
import config from "../config/config";
import type { Command } from "../types/command";
import { logMiniTerminatorRoleChange } from "../utils/logger";
import {
  createMiniTerminatorRequest,
  getMiniTerminatorRequest,
  MINI_TERMINATOR_TTL_MS,
  type MiniTerminatorAction,
  removeMiniTerminatorRequest,
  setMiniTerminatorMessageId,
} from "../utils/pendingMiniTerminators";
import {
  CONFIRMER_ROLE_ID,
  hasAdministratorPermission,
  isTerminator,
} from "../utils/roles";

const ROLE_LABEL = "**Mini-Terminator**";

async function applyMiniTerminatorRoleChange(
  guild: Guild,
  targetId: string,
  action: MiniTerminatorAction,
) {
  let targetMember: GuildMember | null = null;
  try {
    targetMember = await guild.members.fetch(targetId);
  } catch {
    if (action === "remove") {
      return; // target already left the server, nothing to remove
    }
    throw new Error("That user is no longer in the server.");
  }

  const botMember = guild.members.me;
  if (!botMember) {
    throw new Error("I could not determine my own role position.");
  }

  if (targetMember.roles.highest.position >= botMember.roles.highest.position) {
    throw new Error(
      "I cannot manage this user — their role is **higher or equal** to mine.",
    );
  }

  const role = await guild.roles.fetch(config.miniterminatorRoleId);
  if (!role) {
    throw new Error("The Mini-Terminator role was not found.");
  }
  if (!role.editable) {
    throw new Error(
      "The Mini-Terminator role is not editable by me. Move my role above it and grant Manage Roles.",
    );
  }

  if (action === "add") {
    if (!targetMember.roles.cache.has(role.id)) {
      await targetMember.roles.add(role, "6b6t Mini-Terminator add");
    }
  } else if (targetMember.roles.cache.has(role.id)) {
    await targetMember.roles.remove(role, "6b6t Mini-Terminator remove");
  }
}

function buildDisabledRow(requestId: string) {
  return new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder()
      .setCustomId(`mini_approve_${requestId}`)
      .setLabel("Approve")
      .setStyle(ButtonStyle.Success)
      .setEmoji("✅")
      .setDisabled(true),
    new ButtonBuilder()
      .setCustomId(`mini_reject_${requestId}`)
      .setLabel("Reject")
      .setStyle(ButtonStyle.Danger)
      .setEmoji("❌")
      .setDisabled(true),
  );
}

const MiniTerminatorCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("miniterminator")
    .setDescription(
      "Add or remove the Mini-Terminator role (requires second Terminator approval)",
    )
    .addSubcommand((subcommand) =>
      subcommand
        .setName("add")
        .setDescription("Grant the Mini-Terminator role to a user")
        .addUserOption((option) =>
          option
            .setName("user")
            .setDescription("The user to grant the role to")
            .setRequired(true),
        ),
    )
    .addSubcommand((subcommand) =>
      subcommand
        .setName("remove")
        .setDescription("Remove the Mini-Terminator role from a user")
        .addUserOption((option) =>
          option
            .setName("user")
            .setDescription("The user to remove the role from")
            .setRequired(true),
        ),
    ),

  cooldown: 0,

  async execute(interaction: ChatInputCommandInteraction) {
    const member = interaction.member as GuildMember;

    if (!hasAdministratorPermission(member) && !isTerminator(member)) {
      await interaction.reply({
        content:
          "❌ Only members with the **Terminator** role can use this command.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const action: MiniTerminatorAction =
      interaction.options.getSubcommand() === "add" ? "add" : "remove";

    const targetUser = interaction.options.getUser("user", true);

    const guild = interaction.guild;
    if (!guild) return;

    let targetMember: GuildMember | null = null;
    try {
      targetMember = await guild.members.fetch(targetUser.id);
    } catch {}

    if (!targetMember) {
      await interaction.reply({
        content: "❌ That user is not in the server.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const hasRole = targetMember.roles.cache.has(config.miniterminatorRoleId);
    if (action === "add" && hasRole) {
      await interaction.reply({
        content: `❌ ${targetUser} already has the ${ROLE_LABEL} role.`,
        flags: MessageFlags.Ephemeral,
      });
      return;
    }
    if (action === "remove" && !hasRole) {
      await interaction.reply({
        content: `❌ ${targetUser} does not have the ${ROLE_LABEL} role.`,
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    if (hasAdministratorPermission(member)) {
      await interaction.deferReply({ flags: MessageFlags.Ephemeral });

      try {
        await applyMiniTerminatorRoleChange(guild, targetUser.id, action);

        const successEmbed = new EmbedBuilder()
          .setTitle(
            action === "add"
              ? "🤖 Mini-Terminator Granted"
              : "🤖 Mini-Terminator Removed",
          )
          .setDescription(
            `${targetUser} has been ${
              action === "add" ? "granted" : "removed from"
            } the ${ROLE_LABEL} role by ${member} via **admin bypass**.\n` +
              "No second confirmation was needed.",
          )
          .setColor(0x57f287)
          .setThumbnail(targetUser.displayAvatarURL())
          .setTimestamp();

        await interaction.editReply({ embeds: [successEmbed] });

        await logMiniTerminatorRoleChange(interaction.client, {
          guildId: guild.id,
          action,
          submitterTag: member.user.tag,
          submitterId: member.id,
          targetTag: targetUser.tag,
          targetId: targetUser.id,
          adminBypass: true,
        });
      } catch (error) {
        console.error("[MiniTerminator] Admin bypass failed:", error);
        await interaction.editReply({
          content: `❌ Failed to ${
            action === "add" ? "assign" : "remove"
          } the role. Error: \`${(error as Error).message}\``,
        });
      }

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

    const requestId = createMiniTerminatorRequest({
      action,
      submitterId: member.id,
      submitterTag: member.user.tag,
      targetId: targetUser.id,
      targetTag: targetUser.tag,
      guildId: guild.id,
      channelId: voteChannelId as string,
    });

    const expiresAt = Math.floor((Date.now() + MINI_TERMINATOR_TTL_MS) / 1000);

    const confirmEmbed = new EmbedBuilder()
      .setTitle(
        action === "add"
          ? "🤖 Mini-Terminator Grant Request"
          : "🤖 Mini-Terminator Removal Request",
      )
      .setDescription(
        `${member} wants to ${
          action === "add" ? "grant" : "remove"
        } the ${ROLE_LABEL} role ${
          action === "add" ? "to" : "from"
        } ${targetUser}.\n\n` +
          "A **different Terminator** must approve this request.\n" +
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
        {
          name: "Action",
          value: action === "add" ? "➕ Add role" : "➖ Remove role",
          inline: false,
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
        .setCustomId(`mini_approve_${requestId}`)
        .setLabel("Approve")
        .setStyle(ButtonStyle.Success)
        .setEmoji("✅"),
      new ButtonBuilder()
        .setCustomId(`mini_reject_${requestId}`)
        .setLabel("Reject")
        .setStyle(ButtonStyle.Danger)
        .setEmoji("❌"),
    );

    const voteMessage = await voteChannel.send({
      content: `<@&${CONFIRMER_ROLE_ID}> a Mini-Terminator role change needs approval.`,
      allowedMentions: {
        parse: [],
        roles: [CONFIRMER_ROLE_ID],
        users: [],
        repliedUser: false,
      },
      embeds: [confirmEmbed],
      components: [row],
    });

    setMiniTerminatorMessageId(requestId, voteMessage.id);

    await interaction.reply({
      content: `🤖 Your Mini-Terminator role change request for **${targetUser.tag}** has been submitted for approval in ${voteChannel}.`,
      flags: MessageFlags.Ephemeral,
    });
  },

  async handleButton(interaction: ButtonInteraction) {
    const customId = interaction.customId;
    const isApproval = customId.startsWith("mini_approve_");
    const requestId = customId
      .replace("mini_approve_", "")
      .replace("mini_reject_", "");

    const request = getMiniTerminatorRequest(requestId);

    if (!request) {
      await interaction.reply({
        content:
          "⏰ This Mini-Terminator request has **expired** or has already been processed.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const clicker = interaction.member as GuildMember;

    if (!isTerminator(clicker)) {
      await interaction.reply({
        content:
          "❌ Only members with the **Terminator** role can approve or reject Mini-Terminator requests.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    if (!isApproval) {
      removeMiniTerminatorRequest(requestId);

      const originalEmbed = interaction.message.embeds[0];
      if (!originalEmbed) {
        await interaction.reply({
          content:
            "❌ Could not process this request — the original embed is missing.",
          flags: MessageFlags.Ephemeral,
        });
        return;
      }

      const embedFields = originalEmbed.fields;
      const statusIndex = embedFields.findIndex((f) => f.name === "Status");

      const rejectedEmbed = EmbedBuilder.from(originalEmbed)
        .setColor(0x95a5a6)
        .setTitle("❌ Mini-Terminator Request Rejected");

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
          "⚠️ You **cannot approve your own** request. A **different** Terminator must approve it.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const originalEmbed = interaction.message.embeds[0];
    if (!originalEmbed) {
      await interaction.reply({
        content:
          "❌ Could not process this request — the original embed is missing.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    await interaction.deferUpdate();

    try {
      const guild = interaction.guild;
      if (!guild) return;
      await applyMiniTerminatorRoleChange(
        guild,
        request.targetId,
        request.action,
      );

      removeMiniTerminatorRequest(requestId);

      const embedFields = originalEmbed.fields;
      const statusIndex = embedFields.findIndex((f) => f.name === "Status");

      const approvedEmbed = EmbedBuilder.from(originalEmbed)
        .setColor(0x57f287)
        .setTitle(
          request.action === "add"
            ? "🤖 Mini-Terminator Granted"
            : "🤖 Mini-Terminator Removed",
        );

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

      await logMiniTerminatorRoleChange(interaction.client, {
        guildId: guild.id,
        action: request.action,
        submitterTag: request.submitterTag,
        submitterId: request.submitterId,
        approverTag: clicker.user.tag,
        approverId: clicker.id,
        targetTag: request.targetTag,
        targetId: request.targetId,
        adminBypass: false,
      });
    } catch (error) {
      console.error("[MiniTerminator] Approval failed:", error);
      await interaction.editReply({
        content: `❌ Failed to ${
          request.action === "add" ? "assign" : "remove"
        } the role. Error: \`${(error as Error).message}\``,
        embeds: [],
        components: [],
      });
    }
  },
};

export default MiniTerminatorCommand;
