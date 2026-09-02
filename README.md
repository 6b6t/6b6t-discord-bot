# 6b6t Discord Bot

The 6b6t Discord bot runs the server's commands, role automation, moderation
approvals, scheduled messages, YouTube notifications, and Discord-to-Telegram
crossposting in one Rust process.

The implementation follows the same runtime conventions as Steward: Rust 2024,
Poise and Serenity for Discord, Tokio for background work, typed configuration,
and structured tracing.

## Requirements

- Rust 1.95 or newer
- A Discord application with Server Members, Message Content, and Message
  Reactions intents enabled
- MariaDB access for account linking, role synchronization, and Telegram
  delivery state
- The external 6b6t command services used by player and rank lookups

## Run locally

Create a `.env` file or export the required environment variables, then run:

```bash
cargo run --release
```

The bot registers its commands in the configured 6b6t guild during startup.
Guild registration makes command changes available immediately.

## Configuration

### Core Discord settings

| Variable | Required | Purpose |
| --- | --- | --- |
| `DISCORD_TOKEN` | Yes | Discord bot token |
| `VOTE_CHANNEL_ID` | For approval commands | Channel for two-person moderation approvals |
| `LOG_CHANNEL_ID` | No | Moderation audit log channel |
| `RUST_LOG` | No | Tracing filter, such as `sixbsixt_discord_bot=debug,info` |

Discord application, guild, channel, and role IDs that define the 6b6t server
layout are typed constants in [`src/config.rs`](src/config.rs).

### Community event submissions

The event workflow is enabled only when all three channel IDs and MariaDB are
configured. `EVENTS_CHANNEL_ID` must identify an Announcement channel. The bot
keeps the application message at the bottom, sends valid applications to the
review channel, and records privacy-safe audit entries in the log channel.

| Variable | Required | Purpose |
| --- | --- | --- |
| `EVENTS_CHANNEL_ID` | Together | Public Announcement channel containing approved events and the Apply button |
| `EVENTS_REVIEW_CHANNEL_ID` | Together | Private Terminator/Marketer review channel |
| `EVENTS_LOG_CHANNEL_ID` | Together | Private event audit channel |
| `EVENTS_TEST_USER_ID` | No | Discord user allowed to bypass only the 100-hour check for testing; unset in normal operation |

The bot needs View Channel, Read Message History, Send Messages, Embed Links,
Manage Messages, and View Audit Log. It must also be able to mention the Events
role. Approved posts are automatically published 120 minutes after posting;
the MariaDB-backed worker resumes pending posts and publications after restart.

### MariaDB

| Variable | Required | Purpose |
| --- | --- | --- |
| `MYSQL_DB_HOST` | For database features | MariaDB host |
| `MYSQL_DB_PORT` | No | MariaDB port, defaults to `3306` |
| `MYSQL_DB_USER` | With `MYSQL_DB_HOST` | MariaDB user |
| `MYSQL_DB_PASS` | With `MYSQL_DB_HOST` | MariaDB password |
| `MYSQL_DB_LINK` | No | Link database, defaults to `6b6t_link` |
| `MYSQL_DB_LINKS` | No | Legacy fallback name for the link database |
| `MYSQL_DB_STATS` | With `MYSQL_DB_HOST` | Player statistics database |

The process creates the link database and missing link or Telegram tables. It
does not alter the external player statistics schema.

### Minecraft and Hytale services

| Variable | Required | Purpose |
| --- | --- | --- |
| `HTTP_PROXY_COMMAND_SERVICE_BASE_URL` | For player status | Player service base URL |
| `HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN` | For player status | Player service bearer token |
| `HTTP_SLAVE1_COMMAND_SERVICE_BASE_URL` | No | Preferred rank service base URL |
| `HTTP_SLAVE1_COMMAND_SERVICE_ACCESS_TOKEN` | No | Preferred rank service token |
| `HYTALE_QUERY_ENDPOINT_URL` | For `/hytaleplayers` | Hytale query endpoint |
| `HYTALE_QUERY_USERNAME` | For `/hytaleplayers` | Hytale query username |
| `HYTALE_QUERY_PASSWORD` | For `/hytaleplayers` | Hytale query password |

Rank lookup falls back to the proxy command service variables when the slave
variables are not set.

### YouTube and MOTD review

| Variable | Required | Purpose |
| --- | --- | --- |
| `YOUTUBE_API_KEY` | For notifications | YouTube Data API key |
| `MOTD_REVIEW_BOT_SECRET` | For MOTD review | Shared website API secret |
| `MOTD_REVIEW_API_URL` | No | Explicit MOTD review endpoint |
| `WEBSITE_BASE_URL` | No | Website base URL used to derive the review endpoint |

### Anarchy mod analytics

Every hour the bot posts anarchy mod tracking analytics (all-time, today, and
yesterday hits and unique IPs) to a Discord channel. When the backend also
maintains the optional keys below, the report additionally shows the share of
online players and of today's active players using the mod; each percentage
line is omitted while its key is empty or missing.

| Key | Type | Purpose |
| --- | --- | --- |
| `anarchymod:hits:total` | Counter | All-time hits |
| `anarchymod:unique_ips:all_time` | Set | All-time unique IPs |
| `anarchymod:hits:daily:YYYY-MM-DD` | Counter | Daily hits |
| `anarchymod:unique_ips:daily:YYYY-MM-DD` | Set | Daily unique IPs |

The service reads these historical keys from Redis. Current online AnarchyMod
users come from the authenticated `/anarchymod-players` command-service endpoint,
while `/network-players` supplies the total online-player denominator. Analytics
is enabled when both the channel and Redis are configured.

| Variable | Required | Purpose |
| --- | --- | --- |
| `ANARCHY_ANALYTICS_CHANNEL_ID` | For analytics | Channel receiving the hourly report |
| `REDIS_URI` | Either this or host | Full Redis connection string, e.g. `redis://default:pass@host:6379` |
| `REDIS_HOST` | For analytics | Redis host (when `REDIS_URI` is not set); a full `redis://` URI here is also accepted |
| `REDIS_PORT` | No | Redis port, defaults to `6379` |
| `REDIS_PASSWORD` | No | Redis password |
| `REDIS_DB` | No | Redis database number, defaults to `0` |

### Community-event announcements

Set `COMMUNITY_EVENT_ANNOUNCEMENTS_ENABLED=true` and configure the English channel to enable purchase announcements. The bot reads the
website's community-event history from the existing Redis host and posts one message for each unseen
extension. Redis is also used to track the last delivered event so announcements are not duplicated.

| Variable | Required | Purpose |
| --- | --- | --- |
| `COMMUNITY_EVENT_ANNOUNCEMENTS_ENABLED` | To enable | Must be exactly `true`; defaults to disabled |
| `COMMUNITY_EVENT_ANNOUNCEMENT_CHANNEL_ID` | To enable | English Discord channel |
| `COMMUNITY_EVENT_ANNOUNCEMENT_CHANNEL_ID_ES` | No | Spanish Discord channel |
| `COMMUNITY_EVENT_ANNOUNCEMENT_CHANNEL_ID_DE` | No | German Discord channel |
| `COMMUNITY_EVENT_ANNOUNCEMENT_CHANNEL_ID_TR` | No | Turkish Discord channel |
| `COMMUNITY_EVENT_ANNOUNCEMENT_CHANNEL_ID_DUPE` | No | Dedicated English `#dupe-event` channel; backfills existing extension history on first enable |
| `REDIS_HOST` | To enable | Reads the event history and stores the last delivered event |

The bot role needs **View Channel** and **Send Messages** in the configured
channels, plus **Add Reactions** for the automatic fire reaction. General channels checkpoint the
latest existing history entry on first startup, so old purchases are not replayed. The dedicated
`#dupe-event` channel intentionally backfills up to the latest 50 history entries the first time it
is enabled, then uses its own checkpoint for new purchases. It also posts one countdown when each
full remaining hour is crossed. Purchase links in that channel use
`dupe_event_bot-event-extension`; hourly countdown links use
`dupe_event_bot-event-countdown`.

### Telegram crossposting

Crossposting is enabled when both `TELEGRAM_CROSSPOST_BOT_TOKEN` and at least
one route are configured.

```env
TELEGRAM_CROSSPOST_BOT_TOKEN=123456:replace-me
TELEGRAM_CROSSPOST_ROUTES=[{"id":"announcements","discordChannelId":"982190978142195712","telegramChatId":"@org6b6t","telegramThreadId":3721,"utmTopic":"news","utmLanguage":"en"},{"id":"changelog","discordChannelId":"1314292152360112148","telegramChatId":"@org6b6t","telegramThreadId":3724,"utmTopic":"changelog","utmLanguage":"en"}]
```

Each route supports these fields:

| Field | Required | Purpose |
| --- | --- | --- |
| `id` | Yes | Stable route name using letters, numbers, `_`, or `-` |
| `discordChannelId` | Yes | Source Discord channel |
| `telegramChatId` | Yes | Destination numeric chat ID or public `@username` |
| `telegramThreadId` | No | Telegram forum topic ID |
| `includeAuthor` | No | Include the Discord author's name |
| `utmTopic` | No | Telegram topic code used in `org6b6t_<topic>_<slot>`; common announcement route IDs default to `news`, otherwise the route ID is used |
| `utmLanguage` | No | Language code added as `lang`; defaults to `en` |

Optional behavior settings:

```env
TELEGRAM_CROSSPOST_SYNC_EDITS=true
TELEGRAM_CROSSPOST_SYNC_DELETES=false
TELEGRAM_CROSSPOST_BACKFILL_ON_FIRST_RUN=false
TELEGRAM_CROSSPOST_RETRY_ATTEMPTS=6
```

The service stores delivery mappings and route checkpoints in MariaDB. On
restart it retries incomplete deliveries and backfills Discord messages newer
than each route checkpoint. Edit synchronization replaces the previous
Telegram delivery. Delete synchronization remains disabled by default.
Crossposted links on `6b6t.org` are normalized to `www.6b6t.org` and receive
Telegram group UTMs. Existing campaigns are preserved; otherwise shop, vote,
and website campaigns are selected from the URL path. External links and the
6b6t blog are left unchanged. Discord Markdown is rendered as safe Telegram
HTML, including labeled links, emphasis, spoilers, and code.

## Commands

| Command | Access | Purpose |
| --- | --- | --- |
| `/ip` | Everyone | Show Java and Bedrock addresses |
| `/anarchymod` | Everyone | Explain how to join Java Edition with AnarchyMod |
| `/playercount` | Everyone | Show player count and uptime |
| `/version` | Everyone | Show the current Minecraft version |
| `/shop` | Everyone | Link to the 6b6t shop |
| `/boost` | Everyone | Explain Discord boost perks |
| `/hytaleplayers` | Everyone | Show Hytale players and metrics |
| `/getuser` | Administrator | Look up a linked Minecraft account |
| `/banreason` | Moderation roles | Show a user's current ban details |
| `/assignyoutuber` | Marketer or administrator | Assign the in-game YouTube rank and YouTuber Discord role by player name or Discord user |
| `/removeyoutuber` | Marketer or administrator | Remove the in-game YouTube rank and YouTuber Discord role by player name or Discord user |
| `/discordbannerset` | Authorized roles | Set the server banner, invite splash, or Discovery splash with approval or administrator bypass |
| `/terminatorban` | Authorized roles | Ban with approval or administrator bypass |
| `/terminatorunban` | Authorized roles | Unban with approval or administrator bypass |
| `/purge` | Authorized roles | Bulk delete recent messages (optionally from one user) |
| `/miniterminator` | Terminator | Add or remove the Mini-Terminator role with approval |
| `/mediachannelsfreq` | Terminator | Change reminder frequency with approval |

Non-administrator moderation changes require approval from a different member
with the Terminator role. Pending approvals expire after one hour.

## Validate changes

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo build --release --locked
```

## Deploy with Railpack

Railpack detects the root `Cargo.toml`, builds the release binary, and starts
`./bin/sixbsixt-discord-bot`. Configure the environment variables in the
deployment service and deploy the repository root.
