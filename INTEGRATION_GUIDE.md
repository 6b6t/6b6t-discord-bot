# BannerBot — Integration Guide

> Complete reference for integrating the dual-confirmation banner & ban approval system into the main bot. All source code, configurations, and setup instructions included.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [Dependencies](#3-dependencies)
4. [Environment Variables](#4-environment-variables)
5. [Role Configuration](#5-role-configuration)
6. [File Structure](#6-file-structure)
7. [Source Code](#7-source-code)
   - 7.1 [utils/roles.ts](#71-utilsrolests)
   - 7.2 [utils/imageValidator.ts](#72-utilsimagevalidatorts)
   - 7.3 [utils/pendingBanners.ts](#73-utilspendingbannersts)
   - 7.4 [utils/pendingBans.ts](#74-utilspendingbansts)
   - 7.5 [utils/logger.ts](#75-utilsloggerts)
   - 7.6 [commands/discordbannerset.ts](#76-commandsdiscordbannersetts)
   - 7.7 [commands/terminatorban.ts](#77-commandsterminatorbants)
   - 7.8 [index.ts (Main Entry)](#78-indexts-main-entry)
   - 7.9 [register-commands.ts](#79-register-commandsts)
8. [Discord Server Setup](#8-discord-server-setup)
9. [Deployment Steps](#9-deployment-steps)
10. [Data Flow Diagrams](#10-data-flow-diagrams)
11. [Security Model](#11-security-model)
12. [Known Limitations](#12-known-limitations)
13. [Test Scenarios](#13-test-scenarios)

---

## 1. Overview

**BannerBot** provides a dual-confirmation approval system for:

- **Server banner changes** — `/discordbannerset`
- **User bans** — `/terminatorban`

Both commands require a **second authorized user (Terminator)** to approve the action via a button click in a dedicated vote channel. Server administrators can bypass the approval process entirely.

**Key behaviors:**

- Submitters cannot approve their own requests
- Pending requests expire after 1 hour (in-memory TTL)
- All successful actions are logged to an audit channel
- Rejections are not logged

---

## 2. Architecture

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  User runs   │     │  Command     │     │  Vote        │
│  /command    │────▶│  validates   │────▶│  channel     │
│  in any      │     │  & creates   │     │  receives    │
│  channel     │     │  pending     │     │  embed with  │
│              │     │  request     │     │  buttons     │
└──────────────┘     └──────────────┘     └──────┬───────┘
                                                  │
                                          ┌───────▼────────┐
                                          │  Terminator    │
                                          │  clicks button │
                                          └───────┬────────┘
                                                  │
                                    ┌─────────────┼─────────────┐
                                    │                           │
                              ┌─────▼─────┐              ┌─────▼─────┐
                              │  Approve  │              │  Reject   │
                              │  Execute  │              │  Cancel   │
                              │  action   │              │  request  │
                              └─────┬─────┘              └───────────┘
                                    │
                              ┌─────▼─────┐
                              │  Audit    │
                              │  log      │
                              └───────────┘
```

**Tech stack:**

- TypeScript 5.7 (ES2022, CommonJS)
- discord.js v14.26.3
- dotenv v16.4.7
- No database — in-memory state only

---

## 3. Dependencies

```json
{
  "dependencies": {
    "discord.js": "^14.16.3",
    "dotenv": "^16.4.7"
  },
  "devDependencies": {
    "@types/node": "^22.10.0",
    "typescript": "^5.7.0"
  }
}
```

**Gateway Intents required:**

- `GatewayIntentBits.Guilds`
- `GatewayIntentBits.GuildMembers` (Server Members Intent — privileged)

---

## 4. Environment Variables

```env
# Bot authentication
DISCORD_TOKEN=your_bot_token_here

# Application ID (for command registration)
CLIENT_ID=your_client_id_here

# Server ID (for guild-scoped command registration)
GUILD_ID=your_guild_id_here

# Audit log channel — successful actions are logged here
LOG_CHANNEL_ID=your_log_channel_id_here

# Vote channel — approval embeds with buttons are posted here
VOTE_CHANNEL_ID=your_vote_channel_id_here
```

| Variable | Required | Used By |
|---|---|---|
| `DISCORD_TOKEN` | Yes | Bot login |
| `CLIENT_ID` | Register only | Slash command registration |
| `GUILD_ID` | Register only | Slash command registration |
| `LOG_CHANNEL_ID` | No (warns if missing) | Audit logging |
| `VOTE_CHANNEL_ID` | Yes (errors if missing) | Vote channel routing |

---

## 5. Role Configuration

**Role matching is done by ID (not name) for reliability.**

| Role | Role ID | Usage |
|---|---|---|
| Terminator | `1268946626387378189` | Can submit requests AND approve/reject |
| Marketer | `1357730279644594399` | Can submit requests only |
| Developer | `1324344058138726481` | Can submit requests only |
| Senior Moderator | `PLACEHOLDER` | Can submit requests only |

> **Note:** Replace `PLACEHOLDER` with the actual Senior Moderator role ID. If this role doesn't exist, remove it from the `AUTHORIZED_ROLE_IDS` array in `roles.ts`.

**Permission tiers:**

| Tier | Who | What they can do |
|---|---|---|
| **Admin bypass** | Users with `Administrator` permission | Execute immediately, no approval needed |
| **Submitter** | Users with any authorized role | Submit requests for approval |
| **Approver** | Users with Terminator role | Approve or reject pending requests |

---

## 6. File Structure

```
src/
├── index.ts                    # Main entry — client, event handlers, button logic
├── register-commands.ts        # CLI script to register slash commands
├── commands/
│   ├── discordbannerset.ts     # /discordbannerset command
│   └── terminatorban.ts        # /terminatorban command
└── utils/
    ├── roles.ts                # Role ID-based permission checks
    ├── imageValidator.ts       # Image attachment/URL validation
    ├── pendingBanners.ts       # In-memory banner request store
    ├── pendingBans.ts          # In-memory ban request store
    └── logger.ts               # Audit logging to Discord channel
```

---

## 7. Source Code

### 7.1 utils/roles.ts

```typescript
import { PermissionFlagsBits, GuildMember } from 'discord.js';

export const AUTHORIZED_ROLE_IDS: string[] = [
  '1268946626387378189',  // Terminator
  'PLACEHOLDER',          // Senior Moderator
  '1357730279644594399',  // Marketer
  '1324344058138726481',  // Developer
];

export const CONFIRMER_ROLE_ID = '1268946626387378189'; // Terminator

export function hasAuthorizedRole(member: GuildMember): boolean {
  return member.roles.cache.some(role =>
    AUTHORIZED_ROLE_IDS.includes(role.id)
  );
}

export function isTerminator(member: GuildMember): boolean {
  return member.roles.cache.has(CONFIRMER_ROLE_ID);
}

export function isAdmin(member: GuildMember): boolean {
  return member.permissions.has(PermissionFlagsBits.Administrator);
}
```

---

### 7.2 utils/imageValidator.ts

```typescript
import { Attachment } from 'discord.js';

export const ALLOWED_CONTENT_TYPES = [
  'image/png',
  'image/jpeg',
  'image/jpg',
  'image/gif',
  'image/webp',
] as const;

export const MAX_FILE_SIZE = 10 * 1024 * 1024; // 10 MB

export interface ValidationResult {
  valid: boolean;
  imageUrl?: string;
  error?: string;
  isAnimated?: boolean;
}

export function validateBannerImage(
  attachment: Attachment | null,
  url: string | null
): ValidationResult {
  if (!attachment && !url) {
    return {
      valid: false,
      error: 'You must provide either an **image attachment** or an **image URL**.',
    };
  }

  if (attachment) {
    const contentType = attachment.contentType?.split(';')[0];
    if (contentType && !(ALLOWED_CONTENT_TYPES as readonly string[]).includes(contentType)) {
      return {
        valid: false,
        error: `Invalid file type: \`${attachment.contentType}\`. Allowed types: PNG, JPG, GIF, WebP.`,
      };
    }

    if (attachment.size > MAX_FILE_SIZE) {
      const sizeMB = (attachment.size / (1024 * 1024)).toFixed(1);
      return {
        valid: false,
        error: `File too large: \`${sizeMB} MB\`. Maximum allowed size is **10 MB**.`,
      };
    }

    const isAnimated =
      attachment.contentType?.includes('gif') || attachment.name?.endsWith('.gif') || false;

    return { valid: true, imageUrl: attachment.url, isAnimated };
  }

  if (url) {
    try {
      const parsed = new URL(url);
      if (!['http:', 'https:'].includes(parsed.protocol)) {
        return { valid: false, error: 'URL must use `http://` or `https://` protocol.' };
      }
    } catch {
      return { valid: false, error: 'Invalid URL format. Please provide a valid image URL.' };
    }

    const lowerUrl = url.toLowerCase().split('?')[0];
    const validExtensions = ['.png', '.jpg', '.jpeg', '.gif', '.webp'];
    const hasValidExtension = validExtensions.some(ext => lowerUrl.endsWith(ext));

    if (!hasValidExtension) {
      return {
        valid: false,
        error: 'URL does not appear to point to a valid image. Supported formats: PNG, JPG, GIF, WebP.',
      };
    }

    const isAnimated = lowerUrl.endsWith('.gif');

    return { valid: true, imageUrl: url, isAnimated };
  }

  return { valid: false, error: 'Unexpected error validating image.' };
}
```

---

### 7.3 utils/pendingBanners.ts

```typescript
import { randomUUID } from 'crypto';

export interface PendingBannerRequest {
  id: string;
  submitterId: string;
  submitterTag: string;
  imageUrl: string;
  guildId: string;
  channelId: string;
  messageId: string | null;
  createdAt: number;
}

export const TTL_MS = 60 * 60 * 1000; // 1 hour

const pendingRequests = new Map<string, PendingBannerRequest>();

export function createRequest(data: {
  submitterId: string;
  submitterTag: string;
  imageUrl: string;
  guildId: string;
  channelId: string;
}): string {
  const id = randomUUID();
  pendingRequests.set(id, {
    id,
    ...data,
    messageId: null,
    createdAt: Date.now(),
  });
  return id;
}

export function getRequest(id: string): PendingBannerRequest | null {
  const request = pendingRequests.get(id);
  if (!request) return null;

  if (Date.now() - request.createdAt > TTL_MS) {
    pendingRequests.delete(id);
    return null;
  }

  return request;
}

export function setMessageId(id: string, messageId: string): void {
  const request = pendingRequests.get(id);
  if (request) {
    request.messageId = messageId;
  }
}

export function removeRequest(id: string): void {
  pendingRequests.delete(id);
}

export function cleanupExpired(): void {
  const now = Date.now();
  for (const [id, request] of pendingRequests) {
    if (now - request.createdAt > TTL_MS) {
      pendingRequests.delete(id);
    }
  }
}

// Auto-cleanup every 10 minutes
setInterval(cleanupExpired, 10 * 60 * 1000).unref();
```

---

### 7.4 utils/pendingBans.ts

```typescript
import { randomUUID } from 'crypto';

export interface PendingBanRequest {
  id: string;
  submitterId: string;
  submitterTag: string;
  targetId: string;
  targetTag: string;
  reason: string;
  deleteMessageDays: number;
  guildId: string;
  channelId: string;
  messageId: string | null;
  createdAt: number;
}

export const BAN_TTL_MS = 60 * 60 * 1000; // 1 hour

const pendingBans = new Map<string, PendingBanRequest>();

export function createBanRequest(data: {
  submitterId: string;
  submitterTag: string;
  targetId: string;
  targetTag: string;
  reason: string;
  deleteMessageDays: number;
  guildId: string;
  channelId: string;
}): string {
  const id = randomUUID();
  pendingBans.set(id, {
    id,
    ...data,
    messageId: null,
    createdAt: Date.now(),
  });
  return id;
}

export function getBanRequest(id: string): PendingBanRequest | null {
  const request = pendingBans.get(id);
  if (!request) return null;

  if (Date.now() - request.createdAt > BAN_TTL_MS) {
    pendingBans.delete(id);
    return null;
  }

  return request;
}

export function setBanMessageId(id: string, messageId: string): void {
  const request = pendingBans.get(id);
  if (request) {
    request.messageId = messageId;
  }
}

export function removeBanRequest(id: string): void {
  pendingBans.delete(id);
}

export function cleanupExpiredBans(): void {
  const now = Date.now();
  for (const [id, request] of pendingBans) {
    if (now - request.createdAt > BAN_TTL_MS) {
      pendingBans.delete(id);
    }
  }
}

// Auto-cleanup every 10 minutes
setInterval(cleanupExpiredBans, 10 * 60 * 1000).unref();
```

---

### 7.5 utils/logger.ts

```typescript
import { Client, EmbedBuilder } from 'discord.js';

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
  opts: BannerChangeLogOptions
): Promise<void> {
  const logChannelId = process.env.LOG_CHANNEL_ID;
  if (!logChannelId) {
    console.warn('[Logger] LOG_CHANNEL_ID not set in .env — skipping audit log.');
    return;
  }

  try {
    const channel = await client.channels.fetch(logChannelId);
    if (!channel || !channel.isTextBased() || !('send' in channel)) {
      console.warn(`[Logger] Channel ${logChannelId} not found or not text-based.`);
      return;
    }

    const embed = new EmbedBuilder()
      .setTitle('🖼️ Server Banner Changed')
      .setColor(0x2b2d31)
      .setImage(opts.imageUrl)
      .setTimestamp()
      .addFields({
        name: 'Submitted By',
        value: `<@${opts.submitterId}> (${opts.submitterTag})`,
        inline: true,
      });

    if (opts.adminBypass) {
      embed.addFields({ name: 'Approved Via', value: '🛡️ Admin Bypass', inline: true });
    } else if (opts.approverTag && opts.approverId) {
      embed.addFields({
        name: 'Approved By',
        value: `<@${opts.approverId}> (${opts.approverTag})`,
        inline: true,
      });
    }

    embed.addFields({
      name: 'Image URL',
      value: `[Click to view](${opts.imageUrl})`,
      inline: false,
    });

    await channel.send({ embeds: [embed] });
  } catch (error) {
    console.error('[Logger] Failed to send audit log:', error);
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
  opts: BanActionLogOptions
): Promise<void> {
  const logChannelId = process.env.LOG_CHANNEL_ID;
  if (!logChannelId) {
    console.warn('[Logger] LOG_CHANNEL_ID not set in .env — skipping audit log.');
    return;
  }

  try {
    const channel = await client.channels.fetch(logChannelId);
    if (!channel || !channel.isTextBased() || !('send' in channel)) {
      console.warn(`[Logger] Channel ${logChannelId} not found or not text-based.`);
      return;
    }

    const embed = new EmbedBuilder()
      .setTitle('🔨 User Banned')
      .setColor(0xed4245)
      .setTimestamp()
      .addFields(
        { name: 'Banned User', value: `<@${opts.targetId}> (${opts.targetTag})`, inline: true },
        { name: 'Banned By', value: `<@${opts.submitterId}> (${opts.submitterTag})`, inline: true },
        { name: 'Reason', value: opts.reason, inline: false },
      );

    if (opts.adminBypass) {
      embed.addFields({ name: 'Approved Via', value: '🛡️ Admin Bypass', inline: true });
    } else if (opts.approverTag && opts.approverId) {
      embed.addFields({
        name: 'Approved By',
        value: `<@${opts.approverId}> (${opts.approverTag})`,
        inline: true,
      });
    }

    await channel.send({ embeds: [embed] });
  } catch (error) {
    console.error('[Logger] Failed to send ban audit log:', error);
  }
}
```

---

### 7.6 commands/discordbannerset.ts

```typescript
import {
  SlashCommandBuilder,
  EmbedBuilder,
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
  GuildPremiumTier,
  ChatInputCommandInteraction,
  GuildMember,
  MessageFlags,
} from 'discord.js';
import { hasAuthorizedRole, isAdmin } from '../utils/roles';
import { validateBannerImage } from '../utils/imageValidator';
import { createRequest, setMessageId, TTL_MS } from '../utils/pendingBanners';
import { logBannerChange } from '../utils/logger';

export const data = new SlashCommandBuilder()
  .setName('discordbannerset')
  .setDescription('Set the server banner (requires second Terminator approval)')
  .addAttachmentOption(option =>
    option
      .setName('image')
      .setDescription('Upload a banner image (PNG, JPG, GIF, WebP — max 10 MB)')
      .setRequired(false)
  )
  .addStringOption(option =>
    option
      .setName('url')
      .setDescription('URL to a hosted banner image')
      .setRequired(false)
  );

export async function execute(interaction: ChatInputCommandInteraction): Promise<void> {
  // ─── 1. Permission Check ──────────────────────────────────────
  const member = interaction.member as GuildMember;

  if (!isAdmin(member) && !hasAuthorizedRole(member)) {
    await interaction.reply({
      content:
        '❌ You do not have permission to use this command. Required roles: **Terminator**, **Senior Moderator**, **Marketer**, **Dev**.',
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  // ─── 2. Input Validation ──────────────────────────────────────
  const attachment = interaction.options.getAttachment('image');
  const url = interaction.options.getString('url');

  const validation = validateBannerImage(attachment, url);
  if (!validation.valid) {
    await interaction.reply({
      content: `❌ ${validation.error}`,
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  const { imageUrl, isAnimated } = validation;

  // ─── 3. Server Boost Level Check ──────────────────────────────
  const guild = interaction.guild!;
  const requiredTier = isAnimated ? GuildPremiumTier.Tier3 : GuildPremiumTier.Tier2;
  const requiredLabel = isAnimated ? 'Boost Level 3' : 'Boost Level 2';

  if (guild.premiumTier < requiredTier) {
    await interaction.reply({
      content: `❌ This server needs **${requiredLabel}** to set ${isAnimated ? 'an animated ' : 'a '}banner. Current tier: **Level ${guild.premiumTier}**.`,
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  // ─── 4. Admin Bypass — Apply Immediately ──────────────────────
  if (isAdmin(member)) {
    await interaction.deferReply();

    try {
      await guild.setBanner(imageUrl!);

      const successEmbed = new EmbedBuilder()
        .setTitle('✅ Server Banner Updated')
        .setDescription(`Banner set by ${member} via **admin bypass** (no confirmation needed).`)
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
      console.error('[BannerSet] Admin bypass failed:', error);
      await interaction.editReply({
        content: `❌ Failed to set the banner. Error: \`${(error as Error).message}\``,
      });
    }

    return;
  }

  // ─── 5. Normal Flow — Requires Second Terminator Confirmation ─
  const voteChannelId = process.env.VOTE_CHANNEL_ID;
  const voteChannel = voteChannelId
    ? await interaction.client.channels.fetch(voteChannelId).catch(() => null)
    : null;

  if (!voteChannel || !voteChannel.isTextBased() || !('send' in voteChannel)) {
    await interaction.reply({
      content: '❌ Vote channel is not configured or not found. Please set `VOTE_CHANNEL_ID` in `.env`.',
      flags: [MessageFlags.Ephemeral],
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
    .setTitle('🖼️ Banner Change Request')
    .setDescription(
      `${member} wants to change the server banner.\n\n` +
      `A **different Terminator** must approve this request.\n` +
      `Expires: <t:${expiresAt}:R>`
    )
    .setImage(imageUrl!)
    .setColor(0xfee75c)
    .addFields(
      { name: 'Submitted By', value: `${member} (${member.user.tag})`, inline: true },
      { name: 'Status', value: '⏳ Awaiting confirmation', inline: true },
    )
    .setFooter({ text: `Request ID: ${requestId}` })
    .setTimestamp();

  const row = new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder()
      .setCustomId(`banner_approve_${requestId}`)
      .setLabel('Approve')
      .setStyle(ButtonStyle.Success)
      .setEmoji('✅'),
    new ButtonBuilder()
      .setCustomId(`banner_reject_${requestId}`)
      .setLabel('Reject')
      .setStyle(ButtonStyle.Danger)
      .setEmoji('❌'),
  );

  const voteMessage = await voteChannel.send({
    embeds: [confirmEmbed],
    components: [row],
  });

  setMessageId(requestId, voteMessage.id);

  await interaction.reply({
    content: `🖼️ Your banner change request has been submitted for approval in ${voteChannel}.`,
    flags: [MessageFlags.Ephemeral],
  });
}
```

---

### 7.7 commands/terminatorban.ts

```typescript
import {
  SlashCommandBuilder,
  EmbedBuilder,
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
  ChatInputCommandInteraction,
  GuildMember,
  PermissionFlagsBits,
  MessageFlags,
} from 'discord.js';
import { hasAuthorizedRole, isAdmin, isTerminator } from '../utils/roles';
import { createBanRequest, setBanMessageId, BAN_TTL_MS } from '../utils/pendingBans';

export const data = new SlashCommandBuilder()
  .setName('terminatorban')
  .setDescription('Ban a user (requires second Terminator approval)')
  .addUserOption(option =>
    option
      .setName('user')
      .setDescription('The user to ban')
      .setRequired(true)
  )
  .addStringOption(option =>
    option
      .setName('reason')
      .setDescription('Reason for the ban')
      .setRequired(false)
  )
  .addIntegerOption(option =>
    option
      .setName('delete_messages')
      .setDescription('Days of messages to delete (0-7)')
      .setRequired(false)
      .setMinValue(0)
      .setMaxValue(7)
  );

export async function execute(interaction: ChatInputCommandInteraction): Promise<void> {
  // ─── 1. Permission Check ──────────────────────────────────────
  const member = interaction.member as GuildMember;

  if (!isAdmin(member) && !hasAuthorizedRole(member)) {
    await interaction.reply({
      content:
        '❌ You do not have permission to use this command. Required roles: **Terminator**, **Senior Moderator**, **Marketer**, **Dev**.',
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  // ─── 2. Parse Options ─────────────────────────────────────────
  const targetUser = interaction.options.getUser('user', true);
  const reason = interaction.options.getString('reason') ?? 'No reason provided';
  const deleteMessageDays = interaction.options.getInteger('delete_messages') ?? 0;

  // ─── 3. Validate Target ───────────────────────────────────────
  const guild = interaction.guild!;

  if (targetUser.id === member.id) {
    await interaction.reply({
      content: '❌ You cannot ban yourself.',
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  if (targetUser.id === interaction.client.user!.id) {
    await interaction.reply({
      content: '❌ I cannot ban myself.',
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  let targetMember: GuildMember | null = null;
  try {
    targetMember = await guild.members.fetch(targetUser.id);
  } catch {
    // User not in server — can still ban by ID
  }

  if (targetMember) {
    if (targetMember.roles.highest.position >= member.roles.highest.position && !isAdmin(member)) {
      await interaction.reply({
        content: '❌ You cannot ban someone with a **higher or equal** role than yours.',
        flags: [MessageFlags.Ephemeral],
      });
      return;
    }

    const botMember = guild.members.me!;
    if (targetMember.roles.highest.position >= botMember.roles.highest.position) {
      await interaction.reply({
        content: '❌ I cannot ban this user — their role is **higher or equal** to mine.',
        flags: [MessageFlags.Ephemeral],
      });
      return;
    }

    if (!targetMember.bannable) {
      await interaction.reply({
        content: '❌ I cannot ban this user. They may be the server owner or have special protections.',
        flags: [MessageFlags.Ephemeral],
      });
      return;
    }
  }

  // ─── 4. Admin Bypass — Ban Immediately ────────────────────────
  if (isAdmin(member)) {
    await interaction.deferReply();

    try {
      await guild.members.ban(targetUser.id, {
        deleteMessageSeconds: deleteMessageDays * 86400,
        reason: `Banned by ${member.user.tag} (admin bypass) — ${reason}`,
      });

      const successEmbed = new EmbedBuilder()
        .setTitle('🔨 User Banned')
        .setDescription(
          `${targetUser} has been banned by ${member} via **admin bypass**.\n` +
          `No second confirmation was needed.`
        )
        .setColor(0xed4245)
        .addFields(
          { name: 'Banned User', value: `${targetUser.tag} (${targetUser.id})`, inline: true },
          { name: 'Reason', value: reason, inline: true },
          { name: 'Messages Deleted', value: `${deleteMessageDays} day(s)`, inline: true },
        )
        .setThumbnail(targetUser.displayAvatarURL())
        .setTimestamp();

      await interaction.editReply({ embeds: [successEmbed] });

      const { logBanAction } = await import('../utils/logger');
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
      console.error('[TerminatorBan] Admin bypass failed:', error);
      await interaction.editReply({
        content: `❌ Failed to ban user. Error: \`${(error as Error).message}\``,
      });
    }

    return;
  }

  // ─── 5. Normal Flow — Requires Second Terminator Confirmation ─
  const voteChannelId = process.env.VOTE_CHANNEL_ID;
  const voteChannel = voteChannelId
    ? await interaction.client.channels.fetch(voteChannelId).catch(() => null)
    : null;

  if (!voteChannel || !voteChannel.isTextBased() || !('send' in voteChannel)) {
    await interaction.reply({
      content: '❌ Vote channel is not configured or not found. Please set `VOTE_CHANNEL_ID` in `.env`.',
      flags: [MessageFlags.Ephemeral],
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
    .setTitle('🔨 Ban Request')
    .setDescription(
      `${member} wants to ban ${targetUser}.\n\n` +
      `A **different Terminator** must approve this request.\n` +
      `Expires: <t:${expiresAt}:R>`
    )
    .setColor(0xfee75c)
    .addFields(
      { name: 'Target', value: `${targetUser.tag} (${targetUser.id})`, inline: true },
      { name: 'Requested By', value: `${member} (${member.user.tag})`, inline: true },
      { name: 'Reason', value: reason, inline: false },
      { name: 'Messages to Delete', value: `${deleteMessageDays} day(s)`, inline: true },
      { name: 'Status', value: '⏳ Awaiting confirmation', inline: true },
    )
    .setThumbnail(targetUser.displayAvatarURL())
    .setFooter({ text: `Request ID: ${requestId}` })
    .setTimestamp();

  const row = new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder()
      .setCustomId(`ban_approve_${requestId}`)
      .setLabel('Approve Ban')
      .setStyle(ButtonStyle.Danger)
      .setEmoji('🔨'),
    new ButtonBuilder()
      .setCustomId(`ban_reject_${requestId}`)
      .setLabel('Reject')
      .setStyle(ButtonStyle.Secondary)
      .setEmoji('❌'),
  );

  const voteMessage = await voteChannel.send({
    embeds: [confirmEmbed],
    components: [row],
  });

  setBanMessageId(requestId, voteMessage.id);

  await interaction.reply({
    content: `🔨 Your ban request for **${targetUser.tag}** has been submitted for approval in ${voteChannel}.`,
    flags: [MessageFlags.Ephemeral],
  });
}
```

---

### 7.8 index.ts (Main Entry)

```typescript
import 'dotenv/config';

import {
  Client,
  GatewayIntentBits,
  Collection,
  EmbedBuilder,
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
  ChatInputCommandInteraction,
  GuildMember,
  Interaction,
  ButtonInteraction,
  MessageFlags,
} from 'discord.js';
import { isTerminator } from './utils/roles';
import { getRequest, removeRequest } from './utils/pendingBanners';
import { getBanRequest, removeBanRequest } from './utils/pendingBans';
import { logBannerChange, logBanAction } from './utils/logger';

import * as discordbannersetCommand from './commands/discordbannerset';
import * as terminatorbanCommand from './commands/terminatorban';

interface BotCommand {
  data: { name: string; toJSON(): unknown };
  execute: (interaction: ChatInputCommandInteraction) => Promise<void>;
}

const client = new Client({
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMembers,
  ],
});

const commands = new Collection<string, BotCommand>();
commands.set(discordbannersetCommand.data.name, discordbannersetCommand);
commands.set(terminatorbanCommand.data.name, terminatorbanCommand);

client.once('clientReady', () => {
  console.log(`✅ BannerBot is online as ${client.user!.tag}`);
  console.log(`📋 Serving ${client.guilds.cache.size} guild(s)`);
});

// ─── Banner Button Handler ───────────────────────────────────────
async function handleBannerButton(interaction: ButtonInteraction, customId: string): Promise<void> {
  const isApproval = customId.startsWith('banner_approve_');
  const requestId = customId
    .replace('banner_approve_', '')
    .replace('banner_reject_', '');

  const request = getRequest(requestId);

  if (!request) {
      await interaction.reply({
        content: '⏰ This banner request has **expired** or has already been processed.',
        flags: [MessageFlags.Ephemeral],
      });
    return;
  }

  const clicker = interaction.member as GuildMember;

  if (!isTerminator(clicker)) {
      await interaction.reply({
        content: '❌ Only members with the **Terminator** role can approve or reject banner requests.',
        flags: [MessageFlags.Ephemeral],
      });
    return;
  }

  // ── Rejection ─────────────────────────────────────────────────
  if (!isApproval) {
    removeRequest(requestId);

    const rejectedEmbed = EmbedBuilder.from(interaction.message.embeds[0])
      .setColor(0xed4245)
      .setTitle('❌ Banner Change Rejected')
      .spliceFields(1, 1, { name: 'Status', value: `❌ Rejected by ${clicker}`, inline: true });

    const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
      new ButtonBuilder()
        .setCustomId(`banner_approve_${requestId}`)
        .setLabel('Approve')
        .setStyle(ButtonStyle.Success)
        .setEmoji('✅')
        .setDisabled(true),
      new ButtonBuilder()
        .setCustomId(`banner_reject_${requestId}`)
        .setLabel('Reject')
        .setStyle(ButtonStyle.Danger)
        .setEmoji('❌')
        .setDisabled(true),
    );

    await interaction.update({ embeds: [rejectedEmbed], components: [disabledRow] });
    return;
  }

  // ── Approval ──────────────────────────────────────────────────
  if (clicker.id === request.submitterId) {
    await interaction.reply({
      content: '⚠️ You **cannot approve your own** banner submission. A **different** Terminator must approve it.',
      flags: [MessageFlags.Ephemeral],
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
      .setTitle('✅ Banner Change Approved')
      .spliceFields(1, 1, { name: 'Status', value: `✅ Approved by ${clicker}`, inline: true });

    const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
      new ButtonBuilder()
        .setCustomId(`banner_approve_${requestId}`)
        .setLabel('Approve')
        .setStyle(ButtonStyle.Success)
        .setEmoji('✅')
        .setDisabled(true),
      new ButtonBuilder()
        .setCustomId(`banner_reject_${requestId}`)
        .setLabel('Reject')
        .setStyle(ButtonStyle.Danger)
        .setEmoji('❌')
        .setDisabled(true),
    );

    await interaction.editReply({ embeds: [approvedEmbed], components: [disabledRow] });

    await logBannerChange(client, {
      guildId: guild.id,
      submitterTag: request.submitterTag,
      submitterId: request.submitterId,
      approverTag: clicker.user.tag,
      approverId: clicker.id,
      imageUrl: request.imageUrl,
      adminBypass: false,
    });
  } catch (error) {
    console.error('[BannerSet] Approval failed:', error);
    await interaction.editReply({
      content: `❌ Failed to set the banner. Error: \`${(error as Error).message}\``,
      embeds: [],
      components: [],
    });
  }
}

// ─── Ban Button Handler ──────────────────────────────────────────
async function handleBanButton(interaction: ButtonInteraction, customId: string): Promise<void> {
  const isApproval = customId.startsWith('ban_approve_');
  const requestId = customId
    .replace('ban_approve_', '')
    .replace('ban_reject_', '');

  const request = getBanRequest(requestId);

  if (!request) {
    await interaction.reply({
      content: '⏰ This ban request has **expired** or has already been processed.',
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  const clicker = interaction.member as GuildMember;

  if (!isTerminator(clicker)) {
    await interaction.reply({
      content: '❌ Only members with the **Terminator** role can approve or reject ban requests.',
      flags: [MessageFlags.Ephemeral],
    });
    return;
  }

  // ── Rejection ─────────────────────────────────────────────────
  if (!isApproval) {
    removeBanRequest(requestId);

    const embedFields = interaction.message.embeds[0].fields;
    const statusIndex = embedFields.findIndex(f => f.name === 'Status');

    const rejectedEmbed = EmbedBuilder.from(interaction.message.embeds[0])
      .setColor(0x95a5a6)
      .setTitle('❌ Ban Request Rejected');

    if (statusIndex !== -1) {
      rejectedEmbed.spliceFields(statusIndex, 1, {
        name: 'Status',
        value: `❌ Rejected by ${clicker}`,
        inline: true,
      });
    }

    const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
      new ButtonBuilder()
        .setCustomId(`ban_approve_${requestId}`)
        .setLabel('Approve Ban')
        .setStyle(ButtonStyle.Danger)
        .setEmoji('🔨')
        .setDisabled(true),
      new ButtonBuilder()
        .setCustomId(`ban_reject_${requestId}`)
        .setLabel('Reject')
        .setStyle(ButtonStyle.Secondary)
        .setEmoji('❌')
        .setDisabled(true),
    );

    await interaction.update({ embeds: [rejectedEmbed], components: [disabledRow] });
    return;
  }

  // ── Approval ──────────────────────────────────────────────────
  if (clicker.id === request.submitterId) {
    await interaction.reply({
      content: '⚠️ You **cannot approve your own** ban request. A **different** Terminator must approve it.',
      flags: [MessageFlags.Ephemeral],
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
    const statusIndex = embedFields.findIndex(f => f.name === 'Status');

    const approvedEmbed = EmbedBuilder.from(interaction.message.embeds[0])
      .setColor(0xed4245)
      .setTitle('🔨 Ban Approved & Executed');

    if (statusIndex !== -1) {
      approvedEmbed.spliceFields(statusIndex, 1, {
        name: 'Status',
        value: `✅ Approved by ${clicker}`,
        inline: true,
      });
    }

    const disabledRow = new ActionRowBuilder<ButtonBuilder>().addComponents(
      new ButtonBuilder()
        .setCustomId(`ban_approve_${requestId}`)
        .setLabel('Approve Ban')
        .setStyle(ButtonStyle.Danger)
        .setEmoji('🔨')
        .setDisabled(true),
      new ButtonBuilder()
        .setCustomId(`ban_reject_${requestId}`)
        .setLabel('Reject')
        .setStyle(ButtonStyle.Secondary)
        .setEmoji('❌')
        .setDisabled(true),
    );

    await interaction.editReply({ embeds: [approvedEmbed], components: [disabledRow] });

    await logBanAction(client, {
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
    console.error('[TerminatorBan] Approval failed:', error);
    await interaction.editReply({
      content: `❌ Failed to ban user. Error: \`${(error as Error).message}\``,
      embeds: [],
      components: [],
    });
  }
}

// ─── Interaction Handler ──────────────────────────────────────────
client.on('interactionCreate', async (interaction: Interaction) => {
  if (interaction.isChatInputCommand()) {
    const command = commands.get(interaction.commandName);
    if (!command) return;

    try {
      await command.execute(interaction);
    } catch (error) {
      console.error(`[Command Error] ${interaction.commandName}:`, error);
      if (interaction.replied || interaction.deferred) {
        await interaction.followUp({
          content: '❌ An error occurred while executing this command.',
          flags: [MessageFlags.Ephemeral],
        });
      } else {
        await interaction.reply({
          content: '❌ An error occurred while executing this command.',
          flags: [MessageFlags.Ephemeral],
        });
      }
    }
    return;
  }

  if (interaction.isButton()) {
    const customId = interaction.customId;

    if (customId.startsWith('banner_approve_') || customId.startsWith('banner_reject_')) {
      await handleBannerButton(interaction, customId);
      return;
    }

    if (customId.startsWith('ban_approve_') || customId.startsWith('ban_reject_')) {
      await handleBanButton(interaction, customId);
      return;
    }
  }
});

// ─── Login ────────────────────────────────────────────────────────
const token = process.env.DISCORD_TOKEN;
if (!token) {
  console.error('❌ DISCORD_TOKEN is not set in .env — cannot start bot.');
  process.exit(1);
}

client.login(token);
```

---

### 7.9 register-commands.ts

```typescript
import 'dotenv/config';

import { REST, Routes } from 'discord.js';

import * as discordbannersetCommand from './commands/discordbannerset';
import * as terminatorbanCommand from './commands/terminatorban';

const commands = [
  discordbannersetCommand.data.toJSON(),
  terminatorbanCommand.data.toJSON(),
];

const token = process.env.DISCORD_TOKEN;
const clientId = process.env.CLIENT_ID;
const guildId = process.env.GUILD_ID;

if (!token || !clientId || !guildId) {
  console.error('❌ Missing DISCORD_TOKEN, CLIENT_ID, or GUILD_ID in .env');
  process.exit(1);
}

const rest = new REST({ version: '10' }).setToken(token);

(async () => {
  try {
    console.log(`🔄 Registering ${commands.length} slash command(s)...`);

    const data = await rest.put(
      Routes.applicationGuildCommands(clientId, guildId),
      { body: commands },
    ) as Array<{ name: string }>;

    console.log(`✅ Successfully registered ${data.length} command(s):`);
    data.forEach(cmd => console.log(`   /${cmd.name}`));
  } catch (error) {
    console.error('❌ Failed to register commands:', error);
    process.exit(1);
  }
})();
```

---

## 8. Discord Server Setup

### Channels to Create

| Channel | Purpose | Env Variable |
|---|---|---|
| `#banner-logs` (or similar) | Audit logs for all successful actions | `LOG_CHANNEL_ID` |
| `#voting` (or similar) | Where approval embeds with buttons are posted | `VOTE_CHANNEL_ID` |

### Bot Permissions

The bot requires these permissions:

| Permission | Why |
|---|---|
| Manage Server | To set the server banner |
| Ban Members | To ban users |
| Send Messages | To post embeds in vote & log channels |
| Use Application Commands | To register slash commands |

### Bot Invite URL

Replace `<CLIENT_ID>` with your application ID:

```
https://discord.com/api/oauth2/authorize?client_id=<CLIENT_ID>&permissions=268435488&scope=bot%20applications.commands
```

### Privileged Gateway Intents

Enable in the Discord Developer Portal → Bot tab:

- **Server Members Intent** — required for role checks

### Server Boost Requirements

| Banner Type | Minimum Boost Level |
|---|---|
| Static (PNG, JPG, WebP) | Level 2 (15 boosts) |
| Animated (GIF) | Level 3 (30 boosts) |

---

## 9. Deployment Steps

```bash
# 1. Install dependencies
npm install discord.js dotenv
npm install -D typescript @types/node

# 2. Configure environment
cp .env.example .env
# Edit .env with your actual values

# 3. Build
npx tsc

# 4. Register slash commands (run once, or after changing command definitions)
node dist/register-commands.js

# 5. Start the bot
node dist/index.js
```

---

## 10. Data Flow Diagrams

### Banner Change Flow

```
User runs /discordbannerset in #general
  │
  ├─ Permission check ──── FAIL ──→ ❌ "You do not have permission" (ephemeral)
  ├─ Input validation ──── FAIL ──→ ❌ "Invalid file / missing image" (ephemeral)
  ├─ Boost tier check ──── FAIL ──→ ❌ "Server needs Boost Level X" (ephemeral)
  │
  ├─ User is Admin? ── YES ──→ guild.setBanner()
  │                            ├─ SUCCESS → ✅ embed in #general + audit log
  │                            └─ FAIL → ❌ error message
  │
  └─ User is NOT Admin ──→ Post embed + buttons to #voting
                            └─ Ephemeral confirmation to user in #general

#voting channel receives embed:
  │
  ├─ Terminator clicks [Approve]
  │   ├─ Same user as submitter? ──→ ⚠️ "Cannot approve your own"
  │   └─ Different Terminator ──→ guild.setBanner()
  │       ├─ SUCCESS → ✅ green embed + audit log in #banner-logs
  │       └─ FAIL → ❌ error message
  │
  ├─ Terminator clicks [Reject]
  │   └─ ❌ red embed, buttons disabled, NO audit log
  │
  └─ Request expires (1 hour)
      └─ ⏰ "Expired or already processed"
```

### Ban Flow

```
User runs /terminatorban in #general
  │
  ├─ Permission check ──── FAIL ──→ ❌ "You do not have permission" (ephemeral)
  ├─ Self-ban check ────── FAIL ──→ ❌ "You cannot ban yourself"
  ├─ Bot-ban check ─────── FAIL ──→ ❌ "I cannot ban myself"
  ├─ Role hierarchy check  FAIL ──→ ❌ "Higher or equal role"
  ├─ Bannable check ────── FAIL ──→ ❌ "Cannot ban this user"
  │
  ├─ User is Admin? ── YES ──→ guild.members.ban()
  │                            ├─ SUCCESS → ✅ embed in #general + audit log
  │                            └─ FAIL → ❌ error message
  │
  └─ User is NOT Admin ──→ Post embed + buttons to #voting
                            └─ Ephemeral confirmation to user in #general

#voting channel receives embed:
  │
  ├─ Terminator clicks [Approve Ban]
  │   ├─ Same user as submitter? ──→ ⚠️ "Cannot approve your own"
  │   └─ Different Terminator ──→ guild.members.ban()
  │       ├─ SUCCESS → 🔨 red embed + audit log in #banner-logs
  │       └─ FAIL → ❌ error message
  │
  ├─ Terminator clicks [Reject]
  │   └─ ❌ gray embed, buttons disabled, NO audit log
  │
  └─ Request expires (1 hour)
      └─ ⏰ "Expired or already processed"
```

---

## 11. Security Model

| Mechanism | How It Works |
|---|---|
| **Role gate** | Only Terminator, Senior Moderator, Marketer, or Dev role holders (or Admins) can submit |
| **Dual confirmation** | A **different** Terminator must approve via button click |
| **Self-approval block** | `clicker.id === request.submitterId` check prevents self-approval |
| **Admin bypass** | Administrator permission holders skip the approval flow entirely |
| **Request expiry** | 1-hour TTL on all pending requests, auto-cleanup every 10 minutes |
| **Audit logging** | Every successful action is logged with submitter, approver, and details |
| **No persistence** | Bot restart clears all pending state (orphaned buttons show "expired") |
| **No rejection logging** | Rejected requests are intentionally not recorded |

---

## 12. Known Limitations

| Limitation | Impact | Severity |
|---|---|---|
| In-memory storage | All pending requests lost on bot restart. Orphaned embeds show "expired". | Low |
| URL validation is extension-based | Only checks file extension, doesn't verify the URL resolves to a valid image | Medium |
| No rate limiting | Any authorized user can submit unlimited requests | Low |
| No multi-vote system | Only one other Terminator needs to approve. No quorum or threshold. | Low |
| No notification system | No DM/ping to alert Terminators of pending requests | Medium |
| Self-rejection allowed | The submitter can reject their own request (only self-approval is blocked) | Low |
| Single guild | Bot is designed for one server only | By design |
| Role IDs hardcoded | Role IDs are in source code, not configurable at runtime | Low |

---

## 13. Test Scenarios

### Banner Tests

| # | Scenario | Steps | Expected Result |
|---|---|---|---|
| 1 | No permissions | Run `/discordbannerset` as a user with no authorized roles | Ephemeral: "You do not have permission" |
| 2 | Missing image | Run `/discordbannerset` without attachment or URL | Ephemeral: "You must provide either an image attachment or an image URL" |
| 3 | Invalid file type | Upload a `.txt` file | Ephemeral: "Invalid file type" |
| 4 | File too large | Upload a file > 10 MB | Ephemeral: "File too large" |
| 5 | Insufficient boost | Try on a non-boosted server | Ephemeral: "Server needs Boost Level 2" |
| 6 | Admin bypass | Run as admin with valid image | Banner set immediately, success embed, audit log sent |
| 7 | Normal submit | Run as Terminator with valid image | Ephemeral confirmation to user, embed with buttons in #voting |
| 8 | Approve | Different Terminator clicks Approve | Banner applied, green embed, audit log sent |
| 9 | Self-approve blocked | Submitter clicks their own Approve | Ephemeral: "You cannot approve your own" |
| 10 | Reject | Terminator clicks Reject | Red embed, buttons disabled, no audit log |
| 11 | Expired request | Click button on a request older than 1 hour | Ephemeral: "Expired or already processed" |

### Ban Tests

| # | Scenario | Steps | Expected Result |
|---|---|---|---|
| 12 | Ban yourself | Run `/terminatorban` targeting yourself | Ephemeral: "You cannot ban yourself" |
| 13 | Ban the bot | Target the bot user | Ephemeral: "I cannot ban myself" |
| 14 | Ban higher role | Target someone with a higher role | Ephemeral: "Cannot ban someone with higher role" |
| 15 | Admin ban bypass | Admin bans a user | Ban executed immediately, success embed, audit log |
| 16 | Normal ban submit | Terminator requests ban | Ephemeral confirmation, embed in #voting |
| 17 | Approve ban | Different Terminator approves | Ban executed, red embed, audit log |
| 18 | Reject ban | Terminator rejects | Gray embed, buttons disabled, no audit log |
