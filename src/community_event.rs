use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use redis::AsyncCommands as _;
use serde::Deserialize;

use crate::config::RedisConfig;

const CHECKPOINT_KEY: &str = "community-event:discord:last-history-id";
const HISTORY_KEY: &str = "community:event:dupe-2026:history";
const HISTORY_INDEX_KEY: &str = "community:event:dupe-2026:history-index";
const HISTORY_LIMIT: usize = 50;

#[derive(Clone)]
pub struct CommunityEventService {
    redis: redis::Client,
    channel_id: serenity::ChannelId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryItem {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    username: String,
    purchase_label: String,
    extension_seconds: u64,
}

impl CommunityEventService {
    pub fn new(redis: &RedisConfig, channel_id: serenity::ChannelId) -> Result<Self> {
        let redis = redis::Client::open(redis.connection_url())
            .context("failed to initialize Redis for community-event announcements")?;
        Ok(Self { redis, channel_id })
    }

    pub async fn poll(&self, ctx: &serenity::Context) -> Result<()> {
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .context("failed to connect to Redis for community-event announcements")?;
        let checkpoint: Option<String> = connection
            .get(CHECKPOINT_KEY)
            .await
            .context("failed to read the community-event Discord checkpoint")?;
        let history = self.fetch_history(&mut connection).await?;
        let Some(latest) = history.first() else {
            return Ok(());
        };

        let Some(checkpoint) = checkpoint else {
            connection
                .set::<_, _, ()>(CHECKPOINT_KEY, &latest.id)
                .await
                .context("failed to initialize the community-event Discord checkpoint")?;
            tracing::info!(
                history_id = latest.id,
                "initialized community-event Discord checkpoint"
            );
            return Ok(());
        };

        let Some(checkpoint_position) = history.iter().position(|item| item.id == checkpoint)
        else {
            tracing::warn!(
                checkpoint,
                latest_history_id = latest.id,
                "community-event Discord checkpoint fell outside the recent history window; skipping old entries"
            );
            connection
                .set::<_, _, ()>(CHECKPOINT_KEY, &latest.id)
                .await
                .context("failed to advance the community-event Discord checkpoint")?;
            return Ok(());
        };

        let mut unseen = history[..checkpoint_position].to_vec();
        unseen.reverse();
        for item in unseen {
            if item.kind == "extension" {
                self.announce(ctx, &item).await?;
            }
            connection
                .set::<_, _, ()>(CHECKPOINT_KEY, &item.id)
                .await
                .context("failed to advance the community-event Discord checkpoint")?;
        }
        Ok(())
    }

    async fn fetch_history(
        &self,
        connection: &mut redis::aio::MultiplexedConnection,
    ) -> Result<Vec<HistoryItem>> {
        let ids: Vec<String> = redis::cmd("ZREVRANGE")
            .arg(HISTORY_INDEX_KEY)
            .arg(0)
            .arg(HISTORY_LIMIT - 1)
            .query_async(connection)
            .await
            .context("failed to load the community-event history index")?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let values: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(HISTORY_KEY)
            .arg(&ids)
            .query_async(connection)
            .await
            .context("failed to load community-event history records")?;
        values
            .into_iter()
            .flatten()
            .map(|raw| {
                serde_json::from_str(&raw)
                    .context("Redis contains an invalid community-event history record")
            })
            .collect()
    }

    async fn announce(&self, ctx: &serenity::Context, item: &HistoryItem) -> Result<()> {
        self.channel_id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(render_announcement(item))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send a community-event announcement")?;
        Ok(())
    }
}

fn render_announcement(item: &HistoryItem) -> String {
    let duration = format_duration(item.extension_seconds);
    match item.purchase_label.as_str() {
        "Prime → Elite upgrade" => format!(
            "🎉 **{}** upgraded from **Prime** to **Elite**, extending the **Dupe Event** by **{duration}**!",
            item.username
        ),
        "Prime → Apex upgrade" => format!(
            "🎉 **{}** upgraded from **Prime** to **Apex**, extending the **Dupe Event** by **{duration}**!",
            item.username
        ),
        "Elite → Apex upgrade" => format!(
            "🎉 **{}** upgraded from **Elite** to **Apex**, extending the **Dupe Event** by **{duration}**!",
            item.username
        ),
        label => format!(
            "🎉 **{}** extended the **Dupe Event** by **{duration}** with the **{label} Rank**!",
            item.username
        ),
    }
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let mut parts = Vec::new();
    if hours > 0 {
        parts.push(format!("{hours} hour{}", if hours == 1 { "" } else { "s" }));
    }
    if minutes > 0 {
        parts.push(format!(
            "{minutes} minute{}",
            if minutes == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        return format!("{seconds} second{}", if seconds == 1 { "" } else { "s" });
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, seconds: u64) -> HistoryItem {
        HistoryItem {
            id: "event-id".into(),
            kind: "extension".into(),
            username: "AshiqTasdid".into(),
            purchase_label: label.into(),
            extension_seconds: seconds,
        }
    }

    #[test]
    fn durations_use_hours_and_minutes() {
        assert_eq!(format_duration(2_700), "45 minutes");
        assert_eq!(format_duration(3_600), "1 hour");
        assert_eq!(format_duration(12_600), "3 hours 30 minutes");
    }

    #[test]
    fn rank_purchase_message_names_the_rank() {
        assert_eq!(
            render_announcement(&item("Elite", 3_600)),
            "🎉 **AshiqTasdid** extended the **Dupe Event** by **1 hour** with the **Elite Rank**!"
        );
    }

    #[test]
    fn upgrade_message_uses_upgrade_language() {
        assert_eq!(
            render_announcement(&item("Prime → Elite upgrade", 2_700)),
            "🎉 **AshiqTasdid** upgraded from **Prime** to **Elite**, extending the **Dupe Event** by **45 minutes**!"
        );
    }
}
