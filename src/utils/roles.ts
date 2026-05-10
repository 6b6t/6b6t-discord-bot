import { type GuildMember, PermissionFlagsBits } from "discord.js";

export const AUTHORIZED_ROLE_IDS: string[] = [
  "1268946626387378189", // Terminator
  "1357730279644594399", // Marketer
  "1324344058138726481", // Developer
];

export const CONFIRMER_ROLE_ID = "1268946626387378189"; // Terminator

export function hasAuthorizedRole(member: GuildMember): boolean {
  return member.roles.cache.some((role) =>
    AUTHORIZED_ROLE_IDS.includes(role.id),
  );
}

export function isTerminator(member: GuildMember): boolean {
  return member.roles.cache.has(CONFIRMER_ROLE_ID);
}

export function hasAdministratorPermission(member: GuildMember): boolean {
  return member.permissions.has(PermissionFlagsBits.Administrator);
}

/**
 * @deprecated Use `hasAdministratorPermission()` when checking the Discord
 * Administrator permission bit. This helper does not represent the bot's
 * role-based admin gate (`config.commandAdminRoleId`).
 */
export function isAdmin(member: GuildMember): boolean {
  return hasAdministratorPermission(member);
}
