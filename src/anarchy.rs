use std::fmt::Write as _;

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use serde::Deserialize;

use crate::server::PlayerCounts;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnarchyStats {
    pub total_hits: u64,
    pub unique_all_time: u64,
    pub today: DailyStats,
    pub yesterday: DailyStats,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DailyStats {
    pub date: String,
    pub hits: u64,
    #[serde(rename = "uniques")]
    pub unique: u64,
}

impl AnarchyStats {
    /// Renders the analytics report, showing both shares when bots are reported.
    /// Missing counts keep the unavailable messages; zero denominators show 0%.
    pub fn render(
        &self,
        online_users: Option<u64>,
        online_players: Option<PlayerCounts>,
    ) -> String {
        let mut message = format!(
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
        );
        message.push('\n');
        match (online_users, online_players) {
            (Some(users), Some(players)) if players.bots > 0 => {
                let _ = writeln!(
                    message,
                    "Online: {} out of {} real players use AnarchyMod ({}%)",
                    comma_count(users),
                    comma_count(players.humans),
                    percentage(users, players.humans)
                );
                let _ = writeln!(
                    message,
                    "With bots: {} out of {} online accounts ({}%)",
                    comma_count(users),
                    comma_count(players.total),
                    percentage(users, players.total)
                );
            }
            (Some(users), Some(players)) => {
                let _ = writeln!(
                    message,
                    "Online: {} out of {} online players use AnarchyMod ({}%)",
                    comma_count(users),
                    comma_count(players.total),
                    percentage(users, players.total)
                );
            }
            (Some(users), None) => {
                let _ = writeln!(
                    message,
                    "Online: {} AnarchyMod users currently online (player count unavailable)",
                    comma_count(users)
                );
            }
            (None, Some(players)) => {
                let _ = writeln!(
                    message,
                    "Online: AnarchyMod player count unavailable ({} total players online)",
                    comma_count(players.total)
                );
            }
            (None, None) => message.push_str("Online: player counts unavailable\n"),
        }
        message
    }
}

#[derive(Clone)]
pub struct AnarchyService {
    http: reqwest::Client,
    url: String,
    secret: String,
    channel_id: serenity::ChannelId,
}

impl AnarchyService {
    pub fn new(
        http: reqwest::Client,
        url: String,
        secret: String,
        channel_id: serenity::ChannelId,
    ) -> Self {
        Self {
            http,
            url,
            secret,
            channel_id,
        }
    }

    pub async fn report(
        &self,
        ctx: &serenity::Context,
        online_users: Option<u64>,
        online_players: Option<PlayerCounts>,
    ) -> Result<()> {
        let stats = self.fetch().await?;
        self.channel_id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(stats.render(online_users, online_players))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send anarchy mod analytics")?;
        Ok(())
    }

    pub async fn fetch(&self) -> Result<AnarchyStats> {
        self.http
            .get(&self.url)
            .query(&[("resource", "anarchy-mod")])
            .bearer_auth(&self.secret)
            .send()
            .await
            .context("failed to fetch website analytics")?
            .error_for_status()
            .context("website analytics returned an error")?
            .json()
            .await
            .context("website analytics returned invalid data")
    }
}

/// Rounded percentage of `numerator` over `denominator`, clamped to 100.
///
/// Returns `0` when the denominator is zero (backing key absent or empty).
fn percentage(numerator: u64, denominator: u64) -> u8 {
    if denominator == 0 {
        return 0;
    }
    let percent = numerator
        .saturating_mul(100)
        .saturating_add(denominator / 2)
        / denominator;
    percent.min(100) as u8
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
    use super::{AnarchyStats, DailyStats, comma_count, percentage};
    use crate::server::PlayerCounts;

    #[tokio::test]
    async fn fetches_utc_d1_stats_without_redis_and_rejects_errors() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/discord/data", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for status in ["200 OK", "503 Service Unavailable"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let size = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..size]);
                assert!(request.contains("resource=anarchy-mod"));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-secret")
                );
                let body = r#"{"totalHits":123,"uniqueAllTime":42,"today":{"date":"2026-10-02","hits":12,"uniques":6},"yesterday":{"date":"2026-10-01","hits":10,"uniques":5}}"#;
                stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        crate::install_crypto_provider().unwrap();
        let service = super::AnarchyService::new(
            reqwest::Client::new(),
            url,
            "test-secret".into(),
            poise::serenity_prelude::ChannelId::new(1),
        );
        let stats = service.fetch().await.unwrap();
        assert_eq!(stats.total_hits, 123);
        assert_eq!(stats.today.date, "2026-10-02");
        assert_eq!(stats.today.unique, 6);
        assert!(service.fetch().await.is_err());
        server.await.unwrap();
    }

    #[test]
    fn counters_are_grouped_with_thousands_separators() {
        assert_eq!(comma_count(0), "0");
        assert_eq!(comma_count(999), "999");
        assert_eq!(comma_count(1_000), "1,000");
        assert_eq!(comma_count(1_234_567), "1,234,567");
    }

    #[test]
    fn percentages_round_and_clamp() {
        assert_eq!(percentage(45, 371), 12);
        assert_eq!(percentage(1, 3), 33);
        assert_eq!(percentage(3, 3), 100);
        assert_eq!(percentage(5, 3), 100); // clamped, IPs exceed players
    }

    #[test]
    fn percentage_is_zero_without_denominator() {
        assert_eq!(percentage(45, 0), 0);
        assert_eq!(percentage(0, 0), 0);
    }

    fn sample_stats() -> AnarchyStats {
        AnarchyStats {
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
        }
    }

    fn all_players(total: u64) -> PlayerCounts {
        PlayerCounts {
            total,
            humans: total,
            bots: 0,
        }
    }

    #[test]
    fn stats_message_includes_all_reported_metrics() {
        let rendered = sample_stats().render(Some(45), Some(all_players(371)));
        assert!(rendered.contains("1,234,567 hits / 98,765 unique IPs"));
        assert!(rendered.contains("Today (2026-08-08): 3,210 hits / 890 unique IPs"));
        assert!(rendered.contains("Yesterday (2026-08-07): 2,100 hits / 700 unique IPs"));
        assert!(rendered.contains("unique IPs\nOnline:"));
        assert!(rendered.contains("Online: 45 out of 371 online players use AnarchyMod (12%)"));
        assert!(!rendered.contains("Today's players:"));
        assert!(rendered.ends_with("Online: 45 out of 371 online players use AnarchyMod (12%)\n"));
        assert!(!rendered.contains("With bots:"));
    }

    #[test]
    fn online_line_is_always_shown() {
        let stats = sample_stats();
        let rendered = stats.render(Some(0), None);
        assert!(
            rendered
                .contains("Online: 0 AnarchyMod users currently online (player count unavailable)")
        );
        let rendered = stats.render(Some(0), Some(all_players(0)));
        assert!(rendered.contains("Online: 0 out of 0 online players use AnarchyMod (0%)"));
    }

    #[test]
    fn unavailable_anarchymod_endpoint_is_not_reported_as_zero() {
        let rendered = sample_stats().render(None, Some(all_players(371)));
        assert!(
            rendered
                .contains("Online: AnarchyMod player count unavailable (371 total players online)")
        );
        assert!(!rendered.contains("Online: 0 out of 371"));
    }

    #[test]
    fn online_shares_use_real_players_and_all_accounts_when_bots_are_reported() {
        let rendered = sample_stats().render(
            Some(97),
            Some(PlayerCounts {
                total: 620,
                humans: 146,
                bots: 474,
            }),
        );
        assert!(rendered.ends_with(
            "Online: 97 out of 146 real players use AnarchyMod (66%)\n\
             With bots: 97 out of 620 online accounts (16%)\n"
        ));
    }

    #[test]
    fn online_shares_clamp_each_percentage_independently() {
        let players = Some(PlayerCounts {
            total: 620,
            humans: 146,
            bots: 474,
        });
        let rendered = sample_stats().render(Some(200), players);
        assert!(rendered.ends_with(
            "Online: 200 out of 146 real players use AnarchyMod (100%)\n\
             With bots: 200 out of 620 online accounts (32%)\n"
        ));
        let rendered = sample_stats().render(Some(700), players);
        assert!(rendered.ends_with(
            "Online: 700 out of 146 real players use AnarchyMod (100%)\n\
             With bots: 700 out of 620 online accounts (100%)\n"
        ));
    }

    #[test]
    fn online_shares_handle_zero_real_players() {
        let players = Some(PlayerCounts {
            total: 620,
            humans: 0,
            bots: 620,
        });
        let rendered = sample_stats().render(Some(97), players);
        assert!(rendered.ends_with(
            "Online: 97 out of 0 real players use AnarchyMod (0%)\n\
             With bots: 97 out of 620 online accounts (16%)\n"
        ));
        let rendered = sample_stats().render(Some(0), players);
        assert!(rendered.ends_with(
            "Online: 0 out of 0 real players use AnarchyMod (0%)\n\
             With bots: 0 out of 620 online accounts (0%)\n"
        ));
    }

    #[test]
    fn online_shares_keep_thousands_separators() {
        let rendered = sample_stats().render(
            Some(1_000),
            Some(PlayerCounts {
                total: 2_500,
                humans: 1_500,
                bots: 1_000,
            }),
        );
        assert!(rendered.ends_with(
            "Online: 1,000 out of 1,500 real players use AnarchyMod (67%)\n\
             With bots: 1,000 out of 2,500 online accounts (40%)\n"
        ));
    }

    #[test]
    fn unavailable_counts_keep_existing_fallbacks_even_when_bots_are_reported() {
        let stats = sample_stats();
        let rendered = stats.render(
            None,
            Some(PlayerCounts {
                total: 620,
                humans: 146,
                bots: 474,
            }),
        );
        assert!(
            rendered.ends_with(
                "Online: AnarchyMod player count unavailable (620 total players online)\n"
            )
        );
        assert!(!rendered.contains("With bots:"));
        let rendered = stats.render(Some(97), None);
        assert!(rendered.ends_with(
            "Online: 97 AnarchyMod users currently online (player count unavailable)\n"
        ));
        assert!(!rendered.contains("With bots:"));
        assert!(
            stats
                .render(None, None)
                .ends_with("Online: player counts unavailable\n")
        );
    }
}
