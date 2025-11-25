import {
  AuditLogEvent,
  type ChatInputCommandInteraction,
  MessageFlags,
  SlashCommandBuilder,
} from "discord.js";
import type { Command } from "../types/command";

const allowedRoleIds = [
  "1268946626387378189",
  "1268540163068526632",
  "1357730279644594399",
  "1324344058138726481",
  "1349758583859970140",
  "917520262939938915",
];

const BanReasonCommand: Command = {
  data: new SlashCommandBuilder()
    .setName("banreason")
    .setDescription("Show the ban reason for a user")
    .addUserOption((option) =>
      option.setName("user").setDescription("User to check").setRequired(true),
    ),

  async execute(interaction: ChatInputCommandInteraction) {
    if (!interaction.inGuild() || !interaction.guild) {
      await interaction.reply({
        content: "This command can only be used in a server.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const member = await interaction.guild.members.fetch(interaction.user.id);
    const hasPermission = allowedRoleIds.some((id) =>
      member.roles.cache.has(id),
    );

    if (!hasPermission) {
      await interaction.reply({
        content: "You do not have permission to use this command.",
        flags: MessageFlags.Ephemeral,
      });
      return;
    }

    const targetUser = interaction.options.getUser("user", true);
    await interaction.deferReply({ flags: MessageFlags.Ephemeral });

    try {
      const ban = await interaction.guild.bans.fetch(targetUser.id);

      const auditLogs = await interaction.guild.fetchAuditLogs({
        type: AuditLogEvent.MemberBanAdd,
        limit: 10,
      });

      const auditEntry = auditLogs.entries.find(
        (entry) => entry.target?.id === targetUser.id,
      );

      const reason = ban.reason ?? auditEntry?.reason ?? "No reason provided.";
      const bannedBy = auditEntry?.executor?.tag ?? "Unknown (not found)";

      await interaction.editReply(
        `**Ban information for ${targetUser.tag}:**\n` +
          `• Reason: ${reason}\n` +
          `• Banned by: ${bannedBy}`,
      );
    } catch (error) {
      console.error(error);
      await interaction.editReply(
        `${targetUser.tag} is not currently banned from this server.`,
      );
    }
  },
};

export default BanReasonCommand;
