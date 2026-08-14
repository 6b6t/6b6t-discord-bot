use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use poise::serenity_prelude as serenity;
use redis::AsyncCommands as _;
use serde::{Deserialize, Serialize};

use crate::config::{GUILD_ID, RedisConfig};

const CHECKPOINT_KEY: &str = "community-event:discord:last-history-id";
const CHANNEL_LOCK_KEY: &str = "community-event:discord:channel-lock";
const ANNOUNCEMENT_DURATION: Duration = Duration::from_secs(30);
const ANNOUNCEMENT_PERIOD: Duration = Duration::from_secs(5);
const HISTORY_LIMIT: usize = 50;

#[derive(Clone)]
pub struct CommunityEventService {
    http: reqwest::Client,
    redis: redis::Client,
    history_url: String,
    channel_id: serenity::ChannelId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryResponse {
    items: Vec<HistoryItem>,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavedChannelLock {
    had_overwrite: bool,
    allow_bits: u64,
    deny_bits: u64,
    unlock_at_unix: i64,
}

impl CommunityEventService {
    pub fn new(
        http: reqwest::Client,
        redis: &RedisConfig,
        history_url: String,
        channel_id: serenity::ChannelId,
    ) -> Result<Self> {
        reqwest::Url::parse(&history_url).context("COMMUNITY_EVENT_HISTORY_URL must be a URL")?;
        let redis = redis::Client::open(redis.connection_url())
            .context("failed to initialize Redis for community-event announcements")?;
        Ok(Self {
            http,
            redis,
            history_url,
            channel_id,
        })
    }

    pub async fn poll(&self, ctx: &serenity::Context) -> Result<()> {
        self.restore_interrupted_lock(ctx).await?;

        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .context("failed to connect to Redis for community-event announcements")?;
        let checkpoint: Option<String> = connection
            .get(CHECKPOINT_KEY)
            .await
            .context("failed to read the community-event Discord checkpoint")?;
        let history = self.fetch_history().await?;
        let Some(latest) = history.items.first() else {
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

        let Some(checkpoint_position) = history.items.iter().position(|item| item.id == checkpoint)
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

        let mut unseen = history.items[..checkpoint_position].to_vec();
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

    async fn fetch_history(&self) -> Result<HistoryResponse> {
        let mut url = reqwest::Url::parse(&self.history_url)?;
        url.query_pairs_mut()
            .clear()
            .append_pair("limit", &HISTORY_LIMIT.to_string());
        self.http
            .get(url)
            .send()
            .await
            .context("failed to request community-event history")?
            .error_for_status()
            .context("community-event history returned an error")?
            .json()
            .await
            .context("community-event history returned invalid JSON")
    }

    async fn announce(&self, ctx: &serenity::Context, item: &HistoryItem) -> Result<()> {
        let saved_lock = self.lock_channel(ctx).await?;
        let message = render_announcement(item);
        let send_result = async {
            let repetitions = ANNOUNCEMENT_DURATION.as_secs() / ANNOUNCEMENT_PERIOD.as_secs();
            for _ in 0..repetitions {
                self.channel_id
                    .send_message(
                        ctx,
                        serenity::CreateMessage::new()
                            .content(&message)
                            .allowed_mentions(serenity::CreateAllowedMentions::new()),
                    )
                    .await
                    .context("failed to send a community-event announcement")?;
                tokio::time::sleep(ANNOUNCEMENT_PERIOD).await;
            }
            Ok(())
        }
        .await;
        let restore_result = self.restore_lock(ctx, &saved_lock).await;
        send_result.and(restore_result)
    }

    async fn lock_channel(&self, ctx: &serenity::Context) -> Result<SavedChannelLock> {
        let channel = self.guild_channel(ctx).await?;
        let everyone = serenity::PermissionOverwriteType::Role(GUILD_ID.everyone_role());
        let existing = channel
            .permission_overwrites
            .iter()
            .find(|overwrite| overwrite.kind == everyone);
        let saved = SavedChannelLock {
            had_overwrite: existing.is_some(),
            allow_bits: existing.map_or(0, |overwrite| overwrite.allow.bits()),
            deny_bits: existing.map_or(0, |overwrite| overwrite.deny.bits()),
            unlock_at_unix: (chrono::Utc::now()
                + chrono::Duration::from_std(ANNOUNCEMENT_DURATION)?)
            .timestamp(),
        };

        let mut connection = self.redis.get_multiplexed_async_connection().await?;
        connection
            .set::<_, _, ()>(CHANNEL_LOCK_KEY, serde_json::to_string(&saved)?)
            .await
            .context("failed to save the Discord channel lock state")?;

        let mut allow = serenity::Permissions::from_bits_truncate(saved.allow_bits);
        allow.remove(serenity::Permissions::SEND_MESSAGES);
        let mut deny = serenity::Permissions::from_bits_truncate(saved.deny_bits);
        deny.insert(serenity::Permissions::SEND_MESSAGES);
        self.channel_id
            .create_permission(
                ctx,
                serenity::PermissionOverwrite {
                    allow,
                    deny,
                    kind: everyone,
                },
            )
            .await
            .context("failed to lock the community-event announcement channel")?;
        Ok(saved)
    }

    async fn restore_interrupted_lock(&self, ctx: &serenity::Context) -> Result<()> {
        let mut connection = self.redis.get_multiplexed_async_connection().await?;
        let raw: Option<String> = connection.get(CHANNEL_LOCK_KEY).await?;
        let Some(raw) = raw else {
            return Ok(());
        };
        let saved: SavedChannelLock = serde_json::from_str(&raw)
            .context("the saved community-event Discord channel lock is invalid")?;
        let remaining = saved.unlock_at_unix - chrono::Utc::now().timestamp();
        if remaining > 0 {
            tokio::time::sleep(Duration::from_secs(remaining.cast_unsigned())).await;
        }
        self.restore_lock(ctx, &saved).await
    }

    async fn restore_lock(&self, ctx: &serenity::Context, saved: &SavedChannelLock) -> Result<()> {
        let everyone = serenity::PermissionOverwriteType::Role(GUILD_ID.everyone_role());
        if saved.had_overwrite {
            self.channel_id
                .create_permission(
                    ctx,
                    serenity::PermissionOverwrite {
                        allow: serenity::Permissions::from_bits_truncate(saved.allow_bits),
                        deny: serenity::Permissions::from_bits_truncate(saved.deny_bits),
                        kind: everyone,
                    },
                )
                .await
                .context("failed to restore the announcement channel permissions")?;
        } else {
            self.channel_id
                .delete_permission(ctx, everyone)
                .await
                .context("failed to remove the temporary announcement channel permission")?;
        }
        let mut connection = self.redis.get_multiplexed_async_connection().await?;
        connection
            .del::<_, ()>(CHANNEL_LOCK_KEY)
            .await
            .context("failed to clear the saved Discord channel lock")?;
        Ok(())
    }

    async fn guild_channel(&self, ctx: &serenity::Context) -> Result<serenity::GuildChannel> {
        match self.channel_id.to_channel(ctx).await? {
            serenity::Channel::Guild(channel) if channel.guild_id == GUILD_ID => Ok(channel),
            serenity::Channel::Guild(_) => {
                bail!("community-event channel belongs to another guild")
            }
            _ => bail!("community-event announcement target must be a guild channel"),
        }
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
