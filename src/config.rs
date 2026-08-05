use std::env;

use anyhow::{Context as _, Result, bail};
use poise::serenity_prelude as serenity;
use serde::Deserialize;

pub const APPLICATION_ID: u64 = 1_325_506_526_571_532_462;
pub const GUILD_ID: serenity::GuildId = serenity::GuildId::new(917_520_262_797_344_779);
pub const MANUALLY_MANAGED_ROLE_ID: serenity::RoleId =
    serenity::RoleId::new(1_414_830_403_470_233_620);
pub const ADVERTISING_ID: serenity::ChannelId = serenity::ChannelId::new(1_322_175_662_903_005_186);
pub const MERCH_ID: serenity::ChannelId = serenity::ChannelId::new(1_418_951_317_602_172_958);
pub const UPDATES_ID: serenity::ChannelId = serenity::ChannelId::new(982_190_978_142_195_712);
pub const GENERAL_ID: serenity::ChannelId = serenity::ChannelId::new(982_192_297_645_056_040);
pub const YOUTUBE_ID: serenity::ChannelId = serenity::ChannelId::new(1_353_453_116_007_252_050);
pub const COMMAND_ADMIN_ROLE_ID: serenity::RoleId = serenity::RoleId::new(917_520_262_939_938_915);
pub const MINI_TERMINATOR_ROLE_ID: serenity::RoleId =
    serenity::RoleId::new(1_533_970_494_284_365_854);
pub const REVIEW_ID: serenity::ChannelId = serenity::ChannelId::new(1_413_604_129_700_951_372);
pub const ROLE_MENU_ID: serenity::ChannelId = serenity::ChannelId::new(1_418_899_432_790_954_058);
pub const ROLE_MENU_REQUIRED_ROLE_ID: serenity::RoleId =
    serenity::RoleId::new(1_349_026_308_390_391_839);
pub const REACTION_ROLE_MENU_ID: serenity::ChannelId =
    serenity::ChannelId::new(1_330_884_299_615_895_594);
pub const LINKED_ROLE_ID: serenity::RoleId = serenity::RoleId::new(1_325_507_259_307_921_428);
pub const TERMINATOR_ROLE_ID: serenity::RoleId = serenity::RoleId::new(1_268_946_626_387_378_189);

pub const AUTHORIZED_ROLE_IDS: &[serenity::RoleId] = &[
    TERMINATOR_ROLE_ID,
    serenity::RoleId::new(1_357_730_279_644_594_399),
    serenity::RoleId::new(1_324_344_058_138_726_481),
];
pub const BAN_REASON_ROLE_IDS: &[serenity::RoleId] = &[
    TERMINATOR_ROLE_ID,
    serenity::RoleId::new(1_268_540_163_068_526_632),
    serenity::RoleId::new(1_357_730_279_644_594_399),
    serenity::RoleId::new(1_324_344_058_138_726_481),
    serenity::RoleId::new(1_349_758_583_859_970_140),
    COMMAND_ADMIN_ROLE_ID,
];
pub const REVIEW_IGNORE_ROLE_IDS: &[serenity::RoleId] =
    &[COMMAND_ADMIN_ROLE_ID, TERMINATOR_ROLE_ID];
pub const ROLE_MENU_ROLE_IDS: &[serenity::RoleId] = &[
    serenity::RoleId::new(1_418_900_642_356_789_349),
    serenity::RoleId::new(1_418_900_581_464_014_848),
    serenity::RoleId::new(1_418_900_538_363_351_311),
    serenity::RoleId::new(1_418_900_492_850_958_437),
    serenity::RoleId::new(1_418_900_454_422_871_616),
    serenity::RoleId::new(1_418_900_403_566_678_138),
    serenity::RoleId::new(1_418_900_273_325_277_236),
    serenity::RoleId::new(1_418_900_229_020_979_250),
    serenity::RoleId::new(1_418_900_180_039_630_908),
    serenity::RoleId::new(1_418_900_131_339_829_298),
    serenity::RoleId::new(1_418_900_079_452_098_700),
    serenity::RoleId::new(1_418_900_031_695_618_199),
    serenity::RoleId::new(1_418_899_986_778_947_584),
    serenity::RoleId::new(1_418_899_908_148_072_511),
    serenity::RoleId::new(1_418_899_819_006_656_623),
];

pub const REACTION_ROLES: &[(&str, serenity::RoleId)] = &[
    ("✨", serenity::RoleId::new(942_861_111_089_324_142)),
    ("⚔️", serenity::RoleId::new(942_860_042_871_402_567)),
    ("🌩️", serenity::RoleId::new(942_858_847_058_555_000)),
    ("🎉", serenity::RoleId::new(1_155_462_541_871_415_326)),
    ("🏄", serenity::RoleId::new(1_389_335_267_394_982_040)),
    ("🎥", serenity::RoleId::new(1_423_961_521_997_746_227)),
    ("🇺🇸", serenity::RoleId::new(1_051_075_005_250_809_966)),
    ("🇷🇺", serenity::RoleId::new(1_072_504_173_637_144_636)),
    ("🇪🇸", serenity::RoleId::new(1_051_075_060_238_123_078)),
    ("🇹🇷", serenity::RoleId::new(1_330_608_186_436_096_023)),
    ("🇩🇪", serenity::RoleId::new(1_325_150_138_998_754_047)),
    ("🇵🇱", serenity::RoleId::new(1_121_818_071_381_085_107)),
    ("🎮", serenity::RoleId::new(1_461_432_694_041_739_388)),
];

pub const MEDIA_CHANNEL_NAMES: &[&str] = &["screenshots", "memes", "hytale-screenshots"];
pub const MEDIA_CHANNEL_MESSAGE: &str = "To talk in this channel, please link your 6b6t Minecraft account by joining `play.6b6t.org` (anarchy server) and running the command `/link.`";
pub const GENERAL_MESSAGE: &str = "Don't have a rank? Get lower /tpa & /home cooldowns and white username by [voting](<https://www.6b6t.org/vote?utm_source=discord&utm_medium=discord_channel&utm_campaign=evergreen_vote&utm_content=general_bot-vote-reminder&lang=en>)!";
pub const ADVERTISING_MESSAGE: &str = "To post here you need <@&1349026308390391839> or <@&1268345919003430942> or <@&1325147417322192927> or <@&1325147393372586054> rank from the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=evergreen_shop&utm_content=advertising_bot&lang=en>). Ultra ranks are addons on top of the permanent ranks Prime and Elite.";
pub const MERCH_MESSAGE: &str = "Get your 6b6t Mousepad (included with the <@&1268345919003430942> rank) and 6b6t Mug (included with the <@&1349026308390391839> rank) in the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=evergreen_shop&utm_content=merch_bot&lang=en>)!";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramRoute {
    pub id: String,
    pub discord_channel_id: serenity::ChannelId,
    pub telegram_chat_id: String,
    #[serde(default)]
    pub telegram_thread_id: Option<i64>,
    #[serde(default)]
    pub include_author: bool,
}

#[derive(Clone, Debug)]
pub struct TelegramConfig {
    pub token: String,
    pub routes: Vec<TelegramRoute>,
    pub sync_edits: bool,
    pub sync_deletes: bool,
    pub backfill_on_first_run: bool,
    pub retry_attempts: usize,
}

#[derive(Clone, Debug)]
pub struct DatabaseConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub link_database: String,
    pub stats_database: String,
}

#[derive(Clone, Debug)]
pub struct Environment {
    pub discord_token: String,
    pub vote_channel_id: Option<serenity::ChannelId>,
    pub log_channel_id: Option<serenity::ChannelId>,
    pub youtube_api_key: Option<String>,
    pub rank_service_base_url: Option<String>,
    pub rank_service_access_token: Option<String>,
    pub hytale_endpoint_url: Option<String>,
    pub hytale_username: Option<String>,
    pub hytale_password: Option<String>,
    pub motd_review_url: String,
    pub motd_review_secret: Option<String>,
    pub database: Option<DatabaseConfig>,
    pub telegram: Option<TelegramConfig>,
}

impl Environment {
    pub fn load() -> Result<Self> {
        let discord_token =
            env::var("DISCORD_TOKEN").context("DISCORD_TOKEN must be configured")?;
        let telegram = parse_telegram_config()?;
        let database = parse_database_config()?;
        let website =
            env::var("WEBSITE_BASE_URL").unwrap_or_else(|_| "https://www.6b6t.org".into());

        Ok(Self {
            discord_token,
            vote_channel_id: optional_id("VOTE_CHANNEL_ID")?,
            log_channel_id: optional_id("LOG_CHANNEL_ID")?,
            youtube_api_key: optional_env("YOUTUBE_API_KEY"),
            rank_service_base_url: optional_env("HTTP_SLAVE1_COMMAND_SERVICE_BASE_URL")
                .or_else(|| optional_env("HTTP_PROXY_COMMAND_SERVICE_BASE_URL")),
            rank_service_access_token: optional_env("HTTP_SLAVE1_COMMAND_SERVICE_ACCESS_TOKEN")
                .or_else(|| optional_env("HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN")),
            hytale_endpoint_url: optional_env("HYTALE_QUERY_ENDPOINT_URL"),
            hytale_username: optional_env("HYTALE_QUERY_USERNAME"),
            hytale_password: optional_env("HYTALE_QUERY_PASSWORD"),
            motd_review_url: optional_env("MOTD_REVIEW_API_URL").unwrap_or_else(|| {
                format!("{}/api/discord/motd/review", website.trim_end_matches('/'))
            }),
            motd_review_secret: optional_env("MOTD_REVIEW_BOT_SECRET"),
            database,
            telegram,
        })
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn optional_id<T>(name: &str) -> Result<Option<T>>
where
    T: TryFrom<u64>,
    T::Error: std::fmt::Display,
{
    optional_env(name)
        .map(|value| {
            value
                .parse::<u64>()
                .with_context(|| format!("{name} must be a Discord snowflake"))?
                .try_into()
                .map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))
        })
        .transpose()
}

fn parse_database_config() -> Result<Option<DatabaseConfig>> {
    let Some(host) = optional_env("MYSQL_DB_HOST") else {
        return Ok(None);
    };
    let user = env::var("MYSQL_DB_USER").context("MYSQL_DB_USER is required with MYSQL_DB_HOST")?;
    let password =
        env::var("MYSQL_DB_PASS").context("MYSQL_DB_PASS is required with MYSQL_DB_HOST")?;
    let stats_database =
        env::var("MYSQL_DB_STATS").context("MYSQL_DB_STATS is required with MYSQL_DB_HOST")?;
    let port = env::var("MYSQL_DB_PORT")
        .unwrap_or_else(|_| "3306".into())
        .parse()
        .context("MYSQL_DB_PORT must be a port number")?;
    Ok(Some(DatabaseConfig {
        host,
        port,
        user,
        password,
        link_database: optional_env("MYSQL_DB_LINK")
            .or_else(|| optional_env("MYSQL_DB_LINKS"))
            .unwrap_or_else(|| "6b6t_link".into()),
        stats_database,
    }))
}

fn parse_telegram_config() -> Result<Option<TelegramConfig>> {
    let Some(raw_routes) = optional_env("TELEGRAM_CROSSPOST_ROUTES") else {
        return Ok(None);
    };
    let routes: Vec<TelegramRoute> = serde_json::from_str(&raw_routes)
        .context("TELEGRAM_CROSSPOST_ROUTES must be a valid JSON array")?;
    if routes.is_empty() {
        return Ok(None);
    }
    let mut route_ids = std::collections::HashSet::new();
    for route in &routes {
        if route.id.is_empty()
            || route.id.len() > 64
            || !route.id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            bail!("Telegram route IDs may only contain 1-64 letters, numbers, _ and -");
        }
        if !route_ids.insert(&route.id) {
            bail!("duplicate Telegram route ID: {}", route.id);
        }
        if route.telegram_thread_id.is_some_and(|id| id <= 0) {
            bail!("Telegram thread IDs must be positive");
        }
        let valid_numeric_chat = route.telegram_chat_id.len() >= 5
            && route
                .telegram_chat_id
                .strip_prefix('-')
                .unwrap_or(&route.telegram_chat_id)
                .chars()
                .all(|character| character.is_ascii_digit());
        let valid_named_chat = route.telegram_chat_id.starts_with('@')
            && (6..=33).contains(&route.telegram_chat_id.len())
            && route.telegram_chat_id[1..]
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if !valid_numeric_chat && !valid_named_chat {
            bail!("invalid Telegram chat ID for route {}", route.id);
        }
    }
    Ok(Some(TelegramConfig {
        token: env::var("TELEGRAM_CROSSPOST_BOT_TOKEN")
            .context("TELEGRAM_CROSSPOST_BOT_TOKEN is required when routes are configured")?,
        routes,
        sync_edits: env_bool("TELEGRAM_CROSSPOST_SYNC_EDITS", true),
        sync_deletes: env_bool("TELEGRAM_CROSSPOST_SYNC_DELETES", false),
        backfill_on_first_run: env_bool("TELEGRAM_CROSSPOST_BACKFILL_ON_FIRST_RUN", false),
        retry_attempts: optional_env("TELEGRAM_CROSSPOST_RETRY_ATTEMPTS")
            .and_then(|value| value.parse().ok())
            .unwrap_or(6)
            .clamp(1, 12),
    }))
}

fn env_bool(name: &str, fallback: bool) -> bool {
    optional_env(name).map_or(fallback, |value| value.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod tests {
    use super::env_bool;

    #[test]
    fn absent_boolean_uses_fallback() {
        assert!(env_bool("SIXBSIXT_TEST_MISSING_BOOLEAN", true));
    }
}
