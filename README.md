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

### Telegram crossposting

Crossposting is enabled when both `TELEGRAM_CROSSPOST_BOT_TOKEN` and at least
one route are configured.

```env
TELEGRAM_CROSSPOST_BOT_TOKEN=123456:replace-me
TELEGRAM_CROSSPOST_ROUTES=[{"id":"announcements","discordChannelId":"982190978142195712","telegramChatId":"@org6b6t","telegramThreadId":3721},{"id":"changelog","discordChannelId":"1314292152360112148","telegramChatId":"@org6b6t","telegramThreadId":3724}]
```

Each route supports these fields:

| Field | Required | Purpose |
| --- | --- | --- |
| `id` | Yes | Stable route name using letters, numbers, `_`, or `-` |
| `discordChannelId` | Yes | Source Discord channel |
| `telegramChatId` | Yes | Destination numeric chat ID or public `@username` |
| `telegramThreadId` | No | Telegram forum topic ID |
| `includeAuthor` | No | Include the Discord author's name |

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

## Commands

| Command | Access | Purpose |
| --- | --- | --- |
| `/ip` | Everyone | Show Java and Bedrock addresses |
| `/playercount` | Everyone | Show player count and uptime |
| `/version` | Everyone | Show the current Minecraft version |
| `/shop` | Everyone | Link to the 6b6t shop |
| `/boost` | Everyone | Explain Discord boost perks |
| `/hytaleplayers` | Everyone | Show Hytale players and metrics |
| `/getuser` | Administrator | Look up a linked Minecraft account |
| `/banreason` | Moderation roles | Show a user's current ban details |
| `/discordbannerset` | Authorized roles | Set a banner with approval or administrator bypass |
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
