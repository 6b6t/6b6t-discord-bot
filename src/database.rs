use std::time::Duration;

use anyhow::{Context as _, Result};
use sqlx::{MySqlPool, mysql::MySqlPoolOptions};

use crate::config::DatabaseConfig;

#[derive(Clone)]
pub struct Databases {
    pub link: MySqlPool,
    pub stats: MySqlPool,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct LinkMapping {
    pub uuid: String,
    pub discord_id: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct PlayerInfo {
    pub name: String,
    pub first_join_millis: i64,
}

impl Databases {
    pub async fn connect(config: &DatabaseConfig) -> Result<Self> {
        let server_url = format!(
            "mysql://{}:{}@{}:{}",
            encode_url_component(&config.user),
            encode_url_component(&config.password),
            config.host,
            config.port
        );
        let bootstrap = MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&server_url)
            .await
            .context("failed to connect to MySQL")?;
        create_database(&bootstrap, &config.link_database).await?;
        drop(bootstrap);

        let link = connect_database(config, &config.link_database).await?;
        let stats = connect_database(config, &config.stats_database).await?;
        ensure_link_schema(&link).await?;
        Ok(Self { link, stats })
    }

    pub async fn mappings(&self) -> Result<Vec<LinkMapping>> {
        sqlx::query_as::<_, LinkMapping>("SELECT uuid, discord_id FROM uuid_to_discord")
            .fetch_all(&self.link)
            .await
            .context("failed to load linked Discord accounts")
    }

    pub async fn mapping_for_discord(&self, discord_id: &str) -> Result<Option<LinkMapping>> {
        sqlx::query_as::<_, LinkMapping>(
            "SELECT uuid, discord_id FROM uuid_to_discord WHERE discord_id = ? LIMIT 1",
        )
        .bind(discord_id)
        .fetch_optional(&self.link)
        .await
        .context("failed to load Discord account mapping")
    }

    /// Every UUID recorded for `name`, matching case-insensitively regardless
    /// of the stats table collation. Player names can have historical UUIDs, so
    /// callers must resolve ambiguity instead of selecting an arbitrary row.
    pub async fn uuids_for_player_name(&self, name: &str) -> Result<Vec<String>> {
        sqlx::query_scalar::<_, String>("SELECT uuid FROM player_info WHERE LOWER(name) = LOWER(?)")
            .bind(name)
            .fetch_all(&self.stats)
            .await
            .context("failed to look up Minecraft players by name")
    }

    pub async fn mapping_for_uuid(&self, uuid: &str) -> Result<Option<LinkMapping>> {
        sqlx::query_as::<_, LinkMapping>(
            "SELECT uuid, discord_id FROM uuid_to_discord WHERE LOWER(REPLACE(uuid, '-', '')) = ? LIMIT 1",
        )
        .bind(normalize_uuid(uuid))
        .fetch_optional(&self.link)
        .await
        .context("failed to load Discord account mapping")
    }

    pub async fn player_info(&self, uuid: &str) -> Result<Option<PlayerInfo>> {
        sqlx::query_as::<_, PlayerInfo>(
            "SELECT name, first_join AS first_join_millis FROM player_info WHERE uuid = ? LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(&self.stats)
        .await
        .context("failed to load Minecraft player information")
    }
}

fn normalize_uuid(uuid: &str) -> String {
    uuid.chars()
        .filter(|character| *character != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

async fn connect_database(config: &DatabaseConfig, database: &str) -> Result<MySqlPool> {
    let url = format!(
        "mysql://{}:{}@{}:{}/{}",
        encode_url_component(&config.user),
        encode_url_component(&config.password),
        config.host,
        config.port,
        database
    );
    MySqlPoolOptions::new()
        .max_connections(10)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&url)
        .await
        .with_context(|| format!("failed to connect to MySQL database {database}"))
}

async fn create_database(pool: &MySqlPool, database: &str) -> Result<()> {
    if !database
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        anyhow::bail!("MySQL database names may only contain letters, numbers, and underscores");
    }
    let statement = format!(
        "CREATE DATABASE IF NOT EXISTS `{database}` DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
    );
    sqlx::query(sqlx::AssertSqlSafe(statement))
        .execute(pool)
        .await
        .with_context(|| format!("failed to create MySQL database {database}"))?;
    Ok(())
}

async fn ensure_link_schema(pool: &MySqlPool) -> Result<()> {
    for statement in [
        "CREATE TABLE IF NOT EXISTS discord_tokens (user_id VARCHAR(64) NOT NULL PRIMARY KEY, access_token TEXT NOT NULL, refresh_token TEXT NOT NULL, expires_in INT NOT NULL, expires_at BIGINT NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        "CREATE TABLE IF NOT EXISTS discord_role_metadata (user_id VARCHAR(64) NOT NULL PRIMARY KEY, metadata TEXT NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        "CREATE TABLE IF NOT EXISTS uuid_to_discord (uuid CHAR(36) NOT NULL PRIMARY KEY, discord_id VARCHAR(64) NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP, UNIQUE KEY unique_discord_id (discord_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        "CREATE TABLE IF NOT EXISTS telegram_crossposts (route_id VARCHAR(64) NOT NULL, discord_message_id VARCHAR(64) NOT NULL, discord_channel_id VARCHAR(64) NOT NULL, telegram_chat_id VARCHAR(64) NOT NULL, content_hash CHAR(64) NOT NULL, status VARCHAR(16) NOT NULL, telegram_messages LONGTEXT NULL, attempt_count INT NOT NULL DEFAULT 0, last_error TEXT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP, PRIMARY KEY (route_id, discord_message_id), INDEX idx_telegram_crossposts_status (status)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        "CREATE TABLE IF NOT EXISTS telegram_crosspost_routes (route_id VARCHAR(64) NOT NULL PRIMARY KEY, last_discord_message_id VARCHAR(64) NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        "CREATE TABLE IF NOT EXISTS event_submissions (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, submitter_discord_id VARCHAR(64) NOT NULL, linked_uuid CHAR(36) NOT NULL, minecraft_username VARCHAR(16) NOT NULL, event_name VARCHAR(100) NOT NULL, explanation TEXT NOT NULL, discord_invite VARCHAR(512) NOT NULL, promotion_url VARCHAR(512) NOT NULL, event_at BIGINT NOT NULL, event_time_input VARCHAR(64) NOT NULL, join_instructions TEXT NOT NULL, status VARCHAR(24) NOT NULL, denial_reason TEXT NULL, review_message_id VARCHAR(64) NULL, event_message_id VARCHAR(64) NULL, publish_at DATETIME NULL, published_at DATETIME NULL, deleted_at DATETIME NULL, created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP, INDEX idx_event_submitter_status (submitter_discord_id, status), INDEX idx_event_publish (status, publish_at), UNIQUE KEY unique_event_message (event_message_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
        "CREATE TABLE IF NOT EXISTS event_votes (event_id BIGINT UNSIGNED NOT NULL, voter_discord_id VARCHAR(64) NOT NULL, created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY (event_id, voter_discord_id), INDEX idx_event_votes_event (event_id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
    ] {
        sqlx::query(statement)
            .execute(pool)
            .await
            .context("failed to initialize link database schema")?;
    }
    Ok(())
}

fn encode_url_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
