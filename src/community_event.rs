use anyhow::{Context as _, Result};
use chrono::DateTime;
use poise::serenity_prelude as serenity;
use redis::AsyncCommands as _;
use serde::{Deserialize, Serialize};

use crate::config::RedisConfig;

const ENGLISH_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id";
const SPANISH_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id:es";
const GERMAN_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id:de";
const TURKISH_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id:tr";
const DUPE_EVENT_CHECKPOINT_KEY: &str = "community-event:discord:last-history-id:dupe-event";
const DUPE_EVENT_COUNTDOWN_CHECKPOINT_KEY: &str =
    "community-event:discord:dupe-event:countdown-hour-checkpoint";
const EVENT_STATE_KEY: &str = "community:event:dupe-2026:state";
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
    backfill_existing: bool,
    dedicated_dupe_channel: bool,
}

#[derive(Clone)]
pub struct CommunityEventService {
    redis: redis::Client,
    channels: Vec<AnnouncementChannel>,
    dupe_event_channel_id: Option<serenity::ChannelId>,
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

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEventState {
    event_id: String,
    starts_at_ms: i64,
    ends_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CountdownCheckpoint {
    event_id: String,
    ends_at_ms: i64,
    remaining_hours: u64,
}

impl CommunityEventService {
    pub fn new(
        redis: &RedisConfig,
        english_channel_id: serenity::ChannelId,
        spanish_channel_id: Option<serenity::ChannelId>,
        german_channel_id: Option<serenity::ChannelId>,
        turkish_channel_id: Option<serenity::ChannelId>,
        dupe_event_channel_id: Option<serenity::ChannelId>,
    ) -> Result<Self> {
        let redis = redis::Client::open(redis.connection_url())
            .context("failed to initialize Redis for community-event announcements")?;
        let mut channels = vec![AnnouncementChannel {
            id: english_channel_id,
            locale: AnnouncementLocale::English,
            checkpoint_key: ENGLISH_CHECKPOINT_KEY,
            backfill_existing: false,
            dedicated_dupe_channel: false,
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
                    backfill_existing: false,
                    dedicated_dupe_channel: false,
                })
            }),
        );
        let dupe_event_channel_id =
            dupe_event_channel_id.filter(|id| channels.iter().all(|channel| channel.id != *id));
        if let Some(id) = dupe_event_channel_id {
            channels.push(AnnouncementChannel {
                id,
                locale: AnnouncementLocale::English,
                checkpoint_key: DUPE_EVENT_CHECKPOINT_KEY,
                backfill_existing: true,
                dedicated_dupe_channel: true,
            });
        }
        Ok(Self {
            redis,
            channels,
            dupe_event_channel_id,
        })
    }

    pub async fn poll(&self, ctx: &serenity::Context) -> Result<()> {
        let mut connection = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .context("failed to connect to Redis for community-event announcements")?;
        let mut first_error = None;
        if let Some(channel_id) = self.dupe_event_channel_id
            && let Err(error) = self
                .poll_dupe_event_countdown(ctx, &mut connection, channel_id)
                .await
        {
            tracing::error!(
                %error,
                channel_id = channel_id.get(),
                "community-event hourly Discord countdown failed"
            );
            first_error = Some(error);
        }

        let history = self.fetch_history(&mut connection).await?;
        if history.is_empty() {
            for channel in &self.channels {
                connection
                    .set_nx::<_, _, ()>(channel.checkpoint_key, EMPTY_HISTORY_CHECKPOINT)
                    .await
                    .context("failed to initialize an empty community-event checkpoint")?;
            }
            return match first_error {
                Some(error) => Err(error),
                None => Ok(()),
            };
        }
        let Some(latest) = history.first() else {
            return Ok(());
        };

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

        let checkpoint = match checkpoint {
            Some(checkpoint) => checkpoint,
            None if channel.backfill_existing => {
                connection
                    .set::<_, _, ()>(channel.checkpoint_key, EMPTY_HISTORY_CHECKPOINT)
                    .await
                    .context("failed to initialize a community-event backfill checkpoint")?;
                EMPTY_HISTORY_CHECKPOINT.to_owned()
            }
            None => {
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
            }
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

    async fn poll_dupe_event_countdown(
        &self,
        ctx: &serenity::Context,
        connection: &mut redis::aio::MultiplexedConnection,
        channel_id: serenity::ChannelId,
    ) -> Result<()> {
        let state_raw: Option<String> = connection
            .get(EVENT_STATE_KEY)
            .await
            .context("failed to read the community-event state")?;
        let Some(state_raw) = state_raw else {
            return Ok(());
        };
        let state: StoredEventState = serde_json::from_str(&state_raw)
            .context("Redis contains an invalid community-event state")?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let Some(remaining_hours) = countdown_hours_remaining(&state, now_ms) else {
            return Ok(());
        };
        let checkpoint = CountdownCheckpoint {
            event_id: state.event_id.clone(),
            ends_at_ms: state.ends_at_ms,
            remaining_hours,
        };
        let checkpoint_json = serde_json::to_string(&checkpoint)
            .context("failed to serialize the Discord countdown checkpoint")?;
        let previous_raw: Option<String> = connection
            .get(DUPE_EVENT_COUNTDOWN_CHECKPOINT_KEY)
            .await
            .context("failed to read the Discord countdown checkpoint")?;

        let Some(previous_raw) = previous_raw else {
            connection
                .set::<_, _, ()>(DUPE_EVENT_COUNTDOWN_CHECKPOINT_KEY, checkpoint_json)
                .await
                .context("failed to initialize the Discord countdown checkpoint")?;
            return Ok(());
        };
        let previous: CountdownCheckpoint = serde_json::from_str(&previous_raw)
            .context("Redis contains an invalid Discord countdown checkpoint")?;
        if previous.event_id != checkpoint.event_id
            || previous.ends_at_ms != checkpoint.ends_at_ms
            || remaining_hours >= previous.remaining_hours
        {
            if previous.event_id != checkpoint.event_id
                || previous.ends_at_ms != checkpoint.ends_at_ms
                || remaining_hours > previous.remaining_hours
            {
                connection
                    .set::<_, _, ()>(DUPE_EVENT_COUNTDOWN_CHECKPOINT_KEY, checkpoint_json)
                    .await
                    .context("failed to refresh the Discord countdown checkpoint")?;
            }
            return Ok(());
        }

        let delivery_id = format!(
            "{}:{}:{}",
            checkpoint.event_id, checkpoint.ends_at_ms, checkpoint.remaining_hours
        );
        let lock_key = format!("{DUPE_EVENT_COUNTDOWN_CHECKPOINT_KEY}:lock:{delivery_id}");
        let claimed: Option<String> = redis::cmd("SET")
            .arg(&lock_key)
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(60)
            .query_async(connection)
            .await
            .context("failed to claim the Discord countdown delivery")?;
        if claimed.is_none() {
            return Ok(());
        }

        let send_result = channel_id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(render_dupe_event_countdown(remaining_hours))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await;
        if let Err(error) = send_result {
            let _: redis::RedisResult<()> = connection.del(&lock_key).await;
            return Err(error).context("failed to send the hourly Discord countdown");
        }

        redis::pipe()
            .atomic()
            .set(DUPE_EVENT_COUNTDOWN_CHECKPOINT_KEY, checkpoint_json)
            .del(lock_key)
            .query_async::<()>(connection)
            .await
            .context("failed to finalize the Discord countdown delivery")?;
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
        let message = channel
            .id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(render_announcement(
                        item,
                        channel.locale,
                        channel.dedicated_dupe_channel,
                        chrono::Utc::now().timestamp_millis(),
                    )?)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send a community-event announcement")?;
        if let Err(error) = message
            .react(ctx, serenity::ReactionType::Unicode("🔥".to_owned()))
            .await
        {
            tracing::warn!(
                %error,
                channel_id = channel.id.get(),
                message_id = message.id.get(),
                "failed to add the fire reaction to a community-event announcement"
            );
        }
        Ok(())
    }
}

fn countdown_hours_remaining(state: &StoredEventState, now_ms: i64) -> Option<u64> {
    if now_ms < state.starts_at_ms || now_ms >= state.ends_at_ms {
        return None;
    }
    let remaining_ms = u64::try_from(state.ends_at_ms.saturating_sub(now_ms)).ok()?;
    Some(remaining_ms.div_ceil(60 * 60 * 1_000))
}

fn render_dupe_event_countdown(remaining_hours: u64) -> String {
    let remaining = format_remaining(
        remaining_hours.saturating_mul(60 * 60),
        AnnouncementLocale::English,
    );
    let shop_url = "https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=dupe_event_bot-event-countdown&lang=en";
    format!(
        "**{remaining} remain in the Dupe Event.** When the timer reaches 0, the dupe will be disabled.\n\n**Keep the Dupe Event running** - every eligible rank purchase extends the timer. Buy a rank from the [6b6t Shop](<{shop_url}>)."
    )
}

fn render_announcement(
    item: &HistoryItem,
    locale: AnnouncementLocale,
    dedicated_dupe_channel: bool,
    now_ms: i64,
) -> Result<String> {
    let duration = format_duration(item.extension_seconds, locale);
    let remaining = format_remaining(remaining_seconds(item, now_ms)?, locale);
    let upgrade = parse_upgrade(&item.purchase_label);
    let shop_url = shop_url(locale, dedicated_dupe_channel);

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
            "{purchase} and extended the Dupe Event by **{duration}**. The Dupe Event now ends in **{remaining}**.\n\n**Keep the Dupe Event running** - buy a rank from the [6b6t Shop](<{shop_url}>)."
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

fn shop_url(locale: AnnouncementLocale, dedicated_dupe_channel: bool) -> &'static str {
    if dedicated_dupe_channel {
        return "https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=dupe_event_bot-event-extension&lang=en";
    }
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
                false,
                NOW_MS,
            )
            .unwrap(),
            "Player **AshiqTasdid** purchased the **Elite Rank** and extended the Dupe Event by **1 hour**. The Dupe Event now ends in **2 days 3 hours 15 minutes**.\n\n**Keep the Dupe Event running** - buy a rank from the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=en>)."
        );
    }

    #[test]
    fn english_upgrade_matches_the_approved_copy() {
        assert_eq!(
            render_announcement(
                &item("Prime → Elite upgrade", 2_700, 30 * 60),
                AnnouncementLocale::English,
                false,
                NOW_MS,
            )
            .unwrap(),
            "Player **AshiqTasdid** purchased the upgrade from Prime Rank to **Elite Rank** and extended the Dupe Event by **45 minutes**. The Dupe Event now ends in **30 minutes**.\n\n**Keep the Dupe Event running** - buy a rank from the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=general_bot-event-extension&lang=en>)."
        );
    }

    #[test]
    fn localized_messages_use_their_channel_language_and_utm_language() {
        let purchase = item("Elite", 3_600, 30 * 60);
        let spanish =
            render_announcement(&purchase, AnnouncementLocale::Spanish, false, NOW_MS).unwrap();
        let german =
            render_announcement(&purchase, AnnouncementLocale::German, false, NOW_MS).unwrap();
        let turkish =
            render_announcement(&purchase, AnnouncementLocale::Turkish, false, NOW_MS).unwrap();

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

    #[test]
    fn dedicated_channel_has_separate_attribution() {
        let rendered = render_announcement(
            &item("Elite", 3_600, 30 * 60),
            AnnouncementLocale::English,
            true,
            NOW_MS,
        )
        .unwrap();

        assert!(rendered.contains("utm_content=dupe_event_bot-event-extension"));
    }

    #[test]
    fn hourly_countdown_uses_the_dedicated_countdown_attribution() {
        assert_eq!(
            render_dupe_event_countdown(57),
            "**2 days 9 hours remain in the Dupe Event.** When the timer reaches 0, the dupe will be disabled.\n\n**Keep the Dupe Event running** - every eligible rank purchase extends the timer. Buy a rank from the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_dupe_august_2026&utm_content=dupe_event_bot-event-countdown&lang=en>)."
        );
    }

    #[test]
    fn hourly_countdown_changes_only_at_full_remaining_hours() {
        let state = StoredEventState {
            event_id: "dupe-event".into(),
            starts_at_ms: NOW_MS - 1_000,
            ends_at_ms: NOW_MS + 2 * 3_600_000,
        };
        assert_eq!(countdown_hours_remaining(&state, NOW_MS - 1), Some(3));
        assert_eq!(countdown_hours_remaining(&state, NOW_MS), Some(2));
        assert_eq!(countdown_hours_remaining(&state, state.ends_at_ms), None);
    }
}
