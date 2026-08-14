use anyhow::{Context as _, Result};
use chrono::DateTime;
use poise::serenity_prelude as serenity;
use redis::AsyncCommands as _;
use serde::Deserialize;

use crate::config::RedisConfig;

const ENGLISH_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id";
const SPANISH_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id:es";
const GERMAN_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id:de";
const TURKISH_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id:tr";
const HISTORY_KEY: &str = "community:event:dupe-2026:history";
const HISTORY_INDEX_KEY: &str = "community:event:dupe-2026:history-index";
const HISTORY_LIMIT: usize = 50;
const EMPTY_HISTORY_CHECKPOINT: &str = "__empty_history__";

#[derive(Clone, Copy, Debug)]
enum AnnouncementLocale {
    English,
    Spanish,
    German,
    Turkish,
}

#[derive(Clone, Copy)]
struct AnnouncementChannel {
    id: serenity::ChannelId,
    locale: AnnouncementLocale,
    checkpoint_key: &'static str,
}

#[derive(Clone)]
pub struct CommunityEventService {
    redis: redis::Client,
    channels: Vec<AnnouncementChannel>,
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
    resulting_ends_at: String,
}

impl CommunityEventService {
    pub fn new(
        redis: &RedisConfig,
        english_channel_id: serenity::ChannelId,
        spanish_channel_id: Option<serenity::ChannelId>,
        german_channel_id: Option<serenity::ChannelId>,
        turkish_channel_id: Option<serenity::ChannelId>,
    ) -> Result<Self> {
        let redis = redis::Client::open(redis.connection_url())
            .context("failed to initialize Redis for community-event announcements")?;
        let mut channels = vec![AnnouncementChannel {
            id: english_channel_id,
            locale: AnnouncementLocale::English,
            checkpoint_key: ENGLISH_CHECKPOINT_KEY,
        }];
        channels.extend(
            [
                (
                    spanish_channel_id,
                    AnnouncementLocale::Spanish,
                    SPANISH_CHECKPOINT_KEY,
                ),
                (
                    german_channel_id,
                    AnnouncementLocale::German,
                    GERMAN_CHECKPOINT_KEY,
                ),
                (
                    turkish_channel_id,
                    AnnouncementLocale::Turkish,
                    TURKISH_CHECKPOINT_KEY,
                ),
            ]
            .into_iter()
            .filter_map(|(id, locale, checkpoint_key)| {
                id.map(|id| AnnouncementChannel {
                    id,
                    locale,
                    checkpoint_key,
                })
            }),
        );
        Ok(Self { redis, channels })
    }

    pub async fn poll(&self, ctx: &serenity::Context) -> Result<()> {
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .context("failed to connect to Redis for community-event announcements")?;
        let history = self.fetch_history(&mut connection).await?;
        if history.is_empty() {
            for channel in &self.channels {
                connection
                    .set_nx::<_, _, ()>(channel.checkpoint_key, EMPTY_HISTORY_CHECKPOINT)
                    .await
                    .context("failed to initialize an empty community-event checkpoint")?;
            }
            return Ok(());
        }
        let Some(latest) = history.first() else {
            return Ok(());
        };

        let mut first_error = None;
        for channel in &self.channels {
            if let Err(error) = self
                .poll_channel(ctx, &mut connection, *channel, &history, latest)
                .await
            {
                tracing::error!(
                    locale = ?channel.locale,
                    channel_id = channel.id.get(),
                    %error,
                    "community-event localized announcement failed"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    async fn poll_channel(
        &self,
        ctx: &serenity::Context,
        connection: &mut redis::aio::MultiplexedConnection,
        channel: AnnouncementChannel,
        history: &[HistoryItem],
        latest: &HistoryItem,
    ) -> Result<()> {
        let checkpoint: Option<String> = connection
            .get(channel.checkpoint_key)
            .await
            .context("failed to read a community-event Discord checkpoint")?;

        let Some(checkpoint) = checkpoint else {
            connection
                .set::<_, _, ()>(channel.checkpoint_key, &latest.id)
                .await
                .context("failed to initialize a community-event Discord checkpoint")?;
            tracing::info!(
                locale = ?channel.locale,
                channel_id = channel.id.get(),
                history_id = latest.id,
                "initialized community-event Discord checkpoint"
            );
            return Ok(());
        };

        if checkpoint == EMPTY_HISTORY_CHECKPOINT {
            let mut unseen = history.to_vec();
            unseen.reverse();
            for item in unseen {
                if item.kind == "extension" {
                    self.announce(ctx, channel, &item).await?;
                }
                connection
                    .set::<_, _, ()>(channel.checkpoint_key, &item.id)
                    .await
                    .context("failed to advance a community-event Discord checkpoint")?;
            }
            return Ok(());
        }

        let Some(checkpoint_position) = history.iter().position(|item| item.id == checkpoint)
        else {
            tracing::warn!(
                locale = ?channel.locale,
                channel_id = channel.id.get(),
                checkpoint,
                latest_history_id = latest.id,
                "community-event Discord checkpoint fell outside the recent history window; skipping old entries"
            );
            connection
                .set::<_, _, ()>(channel.checkpoint_key, &latest.id)
                .await
                .context("failed to advance a community-event Discord checkpoint")?;
            return Ok(());
        };

        let mut unseen = history[..checkpoint_position].to_vec();
        unseen.reverse();
        for item in unseen {
            if item.kind == "extension" {
                self.announce(ctx, channel, &item).await?;
            }
            connection
                .set::<_, _, ()>(channel.checkpoint_key, &item.id)
                .await
                .context("failed to advance a community-event Discord checkpoint")?;
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

    async fn announce(
        &self,
        ctx: &serenity::Context,
        channel: AnnouncementChannel,
        item: &HistoryItem,
    ) -> Result<()> {
        channel
            .id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(render_announcement(
                        item,
                        channel.locale,
                        chrono::Utc::now().timestamp_millis(),
                    )?)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send a community-event announcement")?;
        Ok(())
    }
}

fn render_announcement(
    item: &HistoryItem,
    locale: AnnouncementLocale,
    now_ms: i64,
) -> Result<String> {
    let duration = format_duration(item.extension_seconds, locale);
    let remaining = format_remaining(remaining_seconds(item, now_ms)?, locale);
    let upgrade = parse_upgrade(&item.purchase_label);
    let shop_url = shop_url(locale);

    let purchase = match (locale, upgrade) {
        (AnnouncementLocale::English, Some((source, target))) => format!(
            "Player **{}** purchased the upgrade from {source} Rank to **{target} Rank**",
            item.username
        ),
        (AnnouncementLocale::English, None) => format!(
            "Player **{}** purchased the **{} Rank**",
            item.username, item.purchase_label
        ),
        (AnnouncementLocale::Spanish, Some((source, target))) => format!(
            "El jugador **{}** compró la mejora del rango {source} al **rango {target}**",
            item.username
        ),
        (AnnouncementLocale::Spanish, None) => format!(
            "El jugador **{}** compró el **rango {}**",
            item.username, item.purchase_label
        ),
        (AnnouncementLocale::German, Some((source, target))) => format!(
            "Spieler **{}** hat das Upgrade vom {source}-Rang auf den **{target}-Rang** gekauft",
            item.username
        ),
        (AnnouncementLocale::German, None) => format!(
            "Spieler **{}** hat den **{}-Rang** gekauft",
            item.username, item.purchase_label
        ),
        (AnnouncementLocale::Turkish, Some((source, target))) => format!(
            "Oyuncu **{}**, {source} Rütbesinden **{target} Rütbesine** yükseltmeyi satın aldı",
            item.username
        ),
        (AnnouncementLocale::Turkish, None) => format!(
            "Oyuncu **{}**, **{} Rütbesini** satın aldı",
            item.username, item.purchase_label
        ),
    };

    let body = match locale {
        AnnouncementLocale::English => format!(
            "{purchase} and the Dupe Event was extended by **{duration}**. Event ends in {remaining}.\n\nIf you want to extend the Dupe Event, buy a rank from the [6b6t Shop](<{shop_url}>)."
        ),
        AnnouncementLocale::Spanish => format!(
            "{purchase} y el Evento de Duplicación se extendió **{duration}**. El evento termina en {remaining}.\n\nSi quieres extender el Evento de Duplicación, compra un rango en la [Tienda de 6b6t](<{shop_url}>)."
        ),
        AnnouncementLocale::German => format!(
            "{purchase} und das Dupe-Event wurde um **{duration}** verlängert. Das Event endet in {remaining}.\n\nWenn du das Dupe-Event verlängern möchtest, kaufe einen Rang im [6b6t-Shop](<{shop_url}>)."
        ),
        AnnouncementLocale::Turkish => format!(
            "{purchase} ve Dupe Etkinliği **{duration}** uzatıldı. Etkinlik {remaining} içinde sona eriyor.\n\nDupe Etkinliğini uzatmak istiyorsan [6b6t Mağazasından](<{shop_url}>) bir rütbe satın al."
        ),
    };
    Ok(body)
}

fn parse_upgrade(label: &str) -> Option<(&str, &str)> {
    label.strip_suffix(" upgrade")?.split_once(" → ")
}

fn remaining_seconds(item: &HistoryItem, now_ms: i64) -> Result<u64> {
    let ends_at_ms = item
        .resulting_ends_at
        .parse::<i64>()
        .or_else(|_| {
            DateTime::parse_from_rfc3339(&item.resulting_ends_at)
                .map(|date| date.timestamp_millis())
        })
        .context("community-event history contains an invalid resulting end time")?;
    let remaining_ms = u64::try_from(ends_at_ms.saturating_sub(now_ms)).unwrap_or(0);
    Ok(remaining_ms.div_ceil(1_000))
}

fn format_duration(seconds: u64, locale: AnnouncementLocale) -> String {
    if seconds < 60 {
        return match locale {
            AnnouncementLocale::English => {
                format!("{seconds} second{}", if seconds == 1 { "" } else { "s" })
            }
            AnnouncementLocale::Spanish => {
                format!("{seconds} segundo{}", if seconds == 1 { "" } else { "s" })
            }
            AnnouncementLocale::German => {
                format!("{seconds} Sekunde{}", if seconds == 1 { "" } else { "n" })
            }
            AnnouncementLocale::Turkish => format!("{seconds} saniye"),
        };
    }
    format_minutes(seconds / 60, locale)
}

fn format_remaining(seconds: u64, locale: AnnouncementLocale) -> String {
    if seconds == 0 {
        return match locale {
            AnnouncementLocale::English => "less than a minute".into(),
            AnnouncementLocale::Spanish => "menos de un minuto".into(),
            AnnouncementLocale::German => "weniger als einer Minute".into(),
            AnnouncementLocale::Turkish => "bir dakikadan az".into(),
        };
    }
    format_minutes(seconds.div_ceil(60), locale)
}

fn format_minutes(total_minutes: u64, locale: AnnouncementLocale) -> String {
    let days = total_minutes / (24 * 60);
    let hours = total_minutes % (24 * 60) / 60;
    let minutes = total_minutes % 60;
    let mut parts = Vec::new();

    if days > 0 {
        parts.push(match locale {
            AnnouncementLocale::English => {
                format!("{days} day{}", if days == 1 { "" } else { "s" })
            }
            AnnouncementLocale::Spanish => {
                format!("{days} día{}", if days == 1 { "" } else { "s" })
            }
            AnnouncementLocale::German => {
                format!("{days} Tag{}", if days == 1 { "" } else { "e" })
            }
            AnnouncementLocale::Turkish => format!("{days} gün"),
        });
    }
    if hours > 0 {
        parts.push(match locale {
            AnnouncementLocale::English => {
                format!("{hours} hour{}", if hours == 1 { "" } else { "s" })
            }
            AnnouncementLocale::Spanish => {
                format!("{hours} hora{}", if hours == 1 { "" } else { "s" })
            }
            AnnouncementLocale::German => {
                format!("{hours} Stunde{}", if hours == 1 { "" } else { "n" })
            }
            AnnouncementLocale::Turkish => format!("{hours} saat"),
        });
    }
    if minutes > 0 {
        parts.push(match locale {
            AnnouncementLocale::English => {
                format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
            }
            AnnouncementLocale::Spanish => {
                format!("{minutes} minuto{}", if minutes == 1 { "" } else { "s" })
            }
            AnnouncementLocale::German => {
                format!("{minutes} Minute{}", if minutes == 1 { "" } else { "n" })
            }
            AnnouncementLocale::Turkish => format!("{minutes} dakika"),
        });
    }

    if parts.is_empty() {
        return format_remaining(0, locale);
    }
    parts.join(" ")
}

fn shop_url(locale: AnnouncementLocale) -> &'static str {
    match locale {
        AnnouncementLocale::English => {
            "https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=en"
        }
        AnnouncementLocale::Spanish => {
            "https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=es"
        }
        AnnouncementLocale::German => {
            "https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=de"
        }
        AnnouncementLocale::Turkish => {
            "https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=tr"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: i64 = 1_000_000_000_000;

    fn item(label: &str, seconds: u64, remaining_seconds: u64) -> HistoryItem {
        HistoryItem {
            id: "event-id".into(),
            kind: "extension".into(),
            username: "AshiqTasdid".into(),
            purchase_label: label.into(),
            extension_seconds: seconds,
            resulting_ends_at: (NOW_MS
                + i64::try_from(remaining_seconds).expect("test duration fits i64") * 1_000)
                .to_string(),
        }
    }

    #[test]
    fn durations_use_hours_and_minutes() {
        assert_eq!(
            format_duration(2_700, AnnouncementLocale::English),
            "45 minutes"
        );
        assert_eq!(
            format_duration(3_600, AnnouncementLocale::English),
            "1 hour"
        );
        assert_eq!(
            format_duration(12_600, AnnouncementLocale::English),
            "3 hours 30 minutes"
        );
    }

    #[test]
    fn remaining_time_omits_zero_units() {
        assert_eq!(
            format_remaining(
                2 * 86_400 + 3 * 3_600 + 15 * 60,
                AnnouncementLocale::English
            ),
            "2 days 3 hours 15 minutes"
        );
        assert_eq!(
            format_remaining(3 * 3_600 + 15 * 60, AnnouncementLocale::English),
            "3 hours 15 minutes"
        );
        assert_eq!(
            format_remaining(30 * 60, AnnouncementLocale::English),
            "30 minutes"
        );
    }

    #[test]
    fn english_rank_purchase_matches_the_approved_copy() {
        assert_eq!(
            render_announcement(
                &item("Elite", 3_600, 2 * 86_400 + 3 * 3_600 + 15 * 60),
                AnnouncementLocale::English,
                NOW_MS,
            )
            .unwrap(),
            "Player **AshiqTasdid** purchased the **Elite Rank** and the Dupe Event was extended by **1 hour**. Event ends in 2 days 3 hours 15 minutes.\n\nIf you want to extend the Dupe Event, buy a rank from the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=en>)."
        );
    }

    #[test]
    fn english_upgrade_matches_the_approved_copy() {
        assert_eq!(
            render_announcement(
                &item("Prime → Elite upgrade", 2_700, 30 * 60),
                AnnouncementLocale::English,
                NOW_MS,
            )
            .unwrap(),
            "Player **AshiqTasdid** purchased the upgrade from Prime Rank to **Elite Rank** and the Dupe Event was extended by **45 minutes**. Event ends in 30 minutes.\n\nIf you want to extend the Dupe Event, buy a rank from the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=en>)."
        );
    }

    #[test]
    fn localized_messages_use_their_channel_language_and_utm_language() {
        let purchase = item("Elite", 3_600, 30 * 60);
        let spanish = render_announcement(&purchase, AnnouncementLocale::Spanish, NOW_MS).unwrap();
        let german = render_announcement(&purchase, AnnouncementLocale::German, NOW_MS).unwrap();
        let turkish = render_announcement(&purchase, AnnouncementLocale::Turkish, NOW_MS).unwrap();

        assert!(spanish.contains("El jugador **AshiqTasdid** compró el **rango Elite**"));
        assert!(spanish.contains("30 minutos"));
        assert!(spanish.contains("&lang=es"));
        assert!(german.contains("Spieler **AshiqTasdid** hat den **Elite-Rang** gekauft"));
        assert!(german.contains("30 Minuten"));
        assert!(german.contains("&lang=de"));
        assert!(turkish.contains("Oyuncu **AshiqTasdid**, **Elite Rütbesini** satın aldı"));
        assert!(turkish.contains("30 dakika"));
        assert!(turkish.contains("&lang=tr"));
    }
}
