use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use redis::AsyncCommands;

use crate::config::RedisConfig;

const TOTAL_HITS_KEY: &str = "anarchymod:hits:total";
const UNIQUE_ALL_TIME_KEY: &str = "anarchymod:unique_ips:all_time";
const DAILY_HITS_PREFIX: &str = "anarchymod:hits:daily:";
const DAILY_UNIQUE_PREFIX: &str = "anarchymod:unique_ips:daily:";

#[derive(Clone, Debug)]
pub struct AnarchyStats {
    pub total_hits: u64,
    pub unique_all_time: u64,
    pub today: DailyStats,
    pub yesterday: DailyStats,
}

#[derive(Clone, Debug)]
pub struct DailyStats {
    pub date: String,
    pub hits: u64,
    pub unique: u64,
}

impl std::fmt::Display for AnarchyStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "**Anarchy Mod Analytics**\n\
             All-time: {} hits / {} unique IPs\n\
             Today ({}): {} hits / {} unique IPs\n\
             Yesterday ({}): {} hits / {} unique IPs",
            comma_count(self.total_hits),
            comma_count(self.unique_all_time),
            self.today.date,
            comma_count(self.today.hits),
            comma_count(self.today.unique),
            self.yesterday.date,
            comma_count(self.yesterday.hits),
            comma_count(self.yesterday.unique),
        )
    }
}

#[derive(Clone)]
pub struct AnarchyService {
    client: redis::Client,
    channel_id: serenity::ChannelId,
}

impl AnarchyService {
    pub fn new(config: &RedisConfig, channel_id: serenity::ChannelId) -> Result<Self> {
        let client = redis::Client::open(config.connection_url())
            .context("failed to build the Redis client")?;
        Ok(Self { client, channel_id })
    }

    pub async fn report(&self, ctx: &serenity::Context) -> Result<()> {
        let stats = self.fetch().await?;
        self.channel_id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(stats.to_string())
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send anarchy mod analytics")?;
        Ok(())
    }

    pub async fn fetch(&self) -> Result<AnarchyStats> {
        let mut connection = self
            .client
            .get_multiplexed_async_connection()
            .await
            .context("failed to connect to Redis")?;
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();

        let total_hits = fetch_counter(&mut connection, TOTAL_HITS_KEY).await?;
        let unique_all_time = connection
            .scard::<_, u64>(UNIQUE_ALL_TIME_KEY)
            .await
            .context("failed to read all-time unique IPs")?;
        let today = DailyStats {
            date: today.clone(),
            hits: fetch_counter(&mut connection, &format!("{DAILY_HITS_PREFIX}{today}")).await?,
            unique: connection
                .scard::<_, u64>(format!("{DAILY_UNIQUE_PREFIX}{today}"))
                .await
                .context("failed to read today's unique IPs")?,
        };
        let yesterday = DailyStats {
            date: yesterday.clone(),
            hits: fetch_counter(&mut connection, &format!("{DAILY_HITS_PREFIX}{yesterday}"))
                .await?,
            unique: connection
                .scard::<_, u64>(format!("{DAILY_UNIQUE_PREFIX}{yesterday}"))
                .await
                .context("failed to read yesterday's unique IPs")?,
        };

        Ok(AnarchyStats {
            total_hits,
            unique_all_time,
            today,
            yesterday,
        })
    }
}

async fn fetch_counter(
    connection: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Result<u64> {
    let value: Option<String> = connection
        .get(key)
        .await
        .context("failed to read analytics counter")?;
    Ok(value.and_then(|value| value.parse().ok()).unwrap_or(0))
}

fn comma_count(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(character);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{AnarchyStats, DailyStats, comma_count};

    #[test]
    fn counters_are_grouped_with_thousands_separators() {
        assert_eq!(comma_count(0), "0");
        assert_eq!(comma_count(999), "999");
        assert_eq!(comma_count(1_000), "1,000");
        assert_eq!(comma_count(1_234_567), "1,234,567");
    }

    #[test]
    fn stats_message_includes_all_reported_metrics() {
        let stats = AnarchyStats {
            total_hits: 1_234_567,
            unique_all_time: 98_765,
            today: DailyStats {
                date: "2026-08-08".into(),
                hits: 3_210,
                unique: 890,
            },
            yesterday: DailyStats {
                date: "2026-08-07".into(),
                hits: 2_100,
                unique: 700,
            },
        };
        let rendered = stats.to_string();
        assert!(rendered.contains("1,234,567 hits / 98,765 unique IPs"));
        assert!(rendered.contains("Today (2026-08-08): 3,210 hits / 890 unique IPs"));
        assert!(rendered.contains("Yesterday (2026-08-07): 2,100 hits / 700 unique IPs"));
    }
}
