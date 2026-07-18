
# 6b6t-discord-bot

Bot for the [6b6t Discord Server](https://discord.6b6t.org)

## Discord Server Features

- **Minecraft Server Info**: Minecraft server IP and player count in the bots status
- **Reaction Role** - Role selection with reactions
- **Role Menu** - Role selection menu for Legend rank role
- **Automatic Reminder**: Send a reminder about ranks in the general channel at 10 AM and 6 PM CET
- **Latest Message**: Makes sure the latest message in #advertising and #6b6t-merch is of the bot
- **YouTube Channel**: Send new youtube videos about 6b6t
- **Sync Linked Users**: Sync linked Minecraft users with their Discord account
- **Telegram Crossposting**: Reliably mirror selected Discord announcement and changelog channels to Telegram

## Commands

| Command | Description | Permissions |
|---------|-------------|-------------|
| `/ip` | Get the 6b6t server IP addresses | Everyone |
| `/playercount` | Check current players online and server uptime | Everyone |
| `/version` | Get the current Minecraft version | Everyone |
| `/shop` | Information about the 6b6t shop | Everyone |
| `/boost` | Information about Discord boosting perks | Everyone |
| `/getuser` | Look up Minecraft account info by Discord user | Administrator |
| `/banreason` | Look up who banned and the user and the reason | Moderator |

## Discord to Telegram crossposting

Crossposting is disabled until both `TELEGRAM_CROSSPOST_BOT_TOKEN` and at
least one route in `TELEGRAM_CROSSPOST_ROUTES` are configured. Routes are a
JSON array:

```env
TELEGRAM_CROSSPOST_BOT_TOKEN=123456:replace-me
TELEGRAM_CROSSPOST_ROUTES=[{"id":"announcements","discordChannelId":"982190978142195712","telegramChatId":"@org6b6t","telegramThreadId":3721},{"id":"changelog","discordChannelId":"1314292152360112148","telegramChatId":"@org6b6t","telegramThreadId":3724}]
```

Route fields:

| Field | Required | Description |
|-------|----------|-------------|
| `id` | Yes | Stable unique route name containing letters, numbers, `_` or `-` |
| `discordChannelId` | Yes | Source Discord text or announcement channel ID |
| `telegramChatId` | Yes | Destination Telegram channel ID (`-100...`) or public `@username` |
| `telegramThreadId` | No | Telegram forum topic ID |
| `includeAuthor` | No | Include the Discord author's display name; defaults to `false` |

Optional settings:

```env
TELEGRAM_CROSSPOST_SYNC_EDITS=true
TELEGRAM_CROSSPOST_SYNC_DELETES=false
TELEGRAM_CROSSPOST_BACKFILL_ON_FIRST_RUN=false
TELEGRAM_CROSSPOST_RETRY_ATTEMPTS=6
```

Crossposts contain the sanitized Discord content directly. Route headings and
Discord/Telegram mentions are not included in Telegram output.

The Discord application must have the **Message Content Intent** enabled in
the Discord Developer Portal. Its guild role needs **View Channel** and
**Read Message History** in every configured source channel.

The Telegram bot must be an administrator in every destination channel with
**Post Messages**. Edit synchronization replaces the old Telegram delivery,
so it also needs **Delete Messages** when edit syncing is enabled. Delete
synchronization is deliberately disabled by default.

Delivery records and route checkpoints are created automatically in the
existing link MariaDB database. They provide duplicate prevention, retries,
restart recovery, edit/delete mapping, and offline message backfill. On the
first deployment, historical messages are not sent unless
`TELEGRAM_CROSSPOST_BACKFILL_ON_FIRST_RUN=true`; with that option enabled,
only the latest existing message per route is posted.
