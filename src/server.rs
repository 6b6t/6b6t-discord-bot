use std::{sync::Arc, time::Duration};

use anyhow::{Context as _, Result, bail};
use chrono::Datelike as _;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{config::Environment, database::Databases};

const SERVER_API: &str = "https://www.6b6t.org/api";
const RANK_FAILURE_THRESHOLD: u8 = 5;

#[derive(Clone, Debug)]
pub struct UserInfo {
    pub top_rank: String,
    pub first_join_year: i32,
}

#[derive(Clone, Debug)]
pub struct ServerData {
    pub players: PlayerCounts,
    pub server_start_unix: Option<i64>,
    pub current_uptime_hours: Option<f64>,
}

/// Network player counts. Player-made bots are accounts the proxy marks as known bots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerCounts {
    /// Everyone online, player-made bots included.
    pub total: u64,
    /// Real players: everyone online except known player-made bots.
    pub humans: u64,
    /// Known player-made bots. Zero when the proxy does not report them.
    pub bots: u64,
}

impl PlayerCounts {
    fn from_response(total: u64, bots: Option<u64>) -> Self {
        let bots = bots.unwrap_or(0).min(total);
        Self {
            total,
            humans: total - bots,
            bots,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HytaleData {
    pub player_count: u64,
    pub max_players: u64,
    pub players: Vec<HytalePlayer>,
    pub metrics: Option<HytaleMetrics>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HytalePlayer {
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct HytaleMetrics {
    pub tps: Option<f64>,
    pub entities: Option<f64>,
    pub chunks: Option<f64>,
}

#[derive(Default)]
struct CircuitBreaker {
    failures: u8,
    open_until: Option<std::time::Instant>,
}

#[derive(Clone)]
pub struct ServerService {
    http: reqwest::Client,
    environment: Arc<Environment>,
    databases: Option<Databases>,
    rank_circuit: Arc<Mutex<CircuitBreaker>>,
}

impl ServerService {
    pub fn new(
        http: reqwest::Client,
        environment: Arc<Environment>,
        databases: Option<Databases>,
    ) -> Self {
        Self {
            http,
            environment,
            databases,
            rank_circuit: Arc::new(Mutex::new(CircuitBreaker::default())),
        }
    }

    pub async fn server_data(&self) -> Result<ServerData> {
        let (players, uptime) = tokio::join!(
            self.players_request("network-players"),
            self.http.get(format!("{SERVER_API}/uptime")).send(),
        );
        let players = players?;
        let uptime = match uptime {
            Ok(response) if response.status().is_success() => response
                .json::<UptimeResponse>()
                .await
                .ok()
                .map(|value| value.statistics),
            _ => None,
        };
        Ok(ServerData {
            players: PlayerCounts::from_response(players.player_count, players.bot_count),
            server_start_unix: uptime.as_ref().and_then(|value| value.server_start_unix),
            current_uptime_hours: uptime.and_then(|value| value.current_uptime_hours),
        })
    }

    async fn player_count_request(&self, endpoint: &str) -> Result<u64> {
        Ok(self.players_request(endpoint).await?.player_count)
    }

    async fn players_request(&self, endpoint: &str) -> Result<PlayersResponse> {
        let base_url = std::env::var("HTTP_PROXY_COMMAND_SERVICE_BASE_URL")
            .context("HTTP_PROXY_COMMAND_SERVICE_BASE_URL is required")?;
        let token = std::env::var("HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN")
            .context("HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN is required")?;
        let response = self
            .http
            .get(format!("{}/{endpoint}", base_url.trim_end_matches('/')))
            .bearer_auth(token)
            .send()
            .await
            .context("players request failed")?;
        if !response.status().is_success() {
            bail!("players request returned HTTP {}", response.status());
        }
        let response: PlayersResponse =
            response.json().await.context("invalid players response")?;
        if !response.success {
            bail!("players service returned an unsuccessful response");
        }
        Ok(response)
    }

    /// Current online counts, used as denominators for anarchy mod analytics.
    pub async fn player_counts(&self) -> Result<PlayerCounts> {
        let players = self.players_request("network-players").await?;
        Ok(PlayerCounts::from_response(
            players.player_count,
            players.bot_count,
        ))
    }

    /// Current online players detected through the `anarchymod:join` plugin message.
    pub async fn anarchymod_player_count(&self) -> Result<u64> {
        self.player_count_request("anarchymod-players").await
    }

    /// Run a `LuckPerms` command against the proxy command service, e.g.
    /// `lpv user <uuid> parent add youtuber`. The player is addressed by UUID so
    /// the assignment survives name changes.
    pub async fn run_lp_command(&self, uuid: &str, action: &str, rank: &str) -> Result<()> {
        let base_url = std::env::var("HTTP_PROXY_COMMAND_SERVICE_BASE_URL")
            .context("HTTP_PROXY_COMMAND_SERVICE_BASE_URL is required")?;
        let token = std::env::var("HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN")
            .context("HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN is required")?;
        let command = format!("lpv user {uuid} parent {action} {rank}");
        let url = format!("{}/run-command", base_url.trim_end_matches('/'));
        let response = match self
            .http
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, token)
            .json(&RunCommandRequest { command: &command })
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::error!(%error, url, command, "proxy command service request failed at the transport layer; check reachability and TLS certificate from the bot host");
                return Err(error).context("command service request failed");
            }
        };
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::error!(url, command, %status, body, "proxy command service returned an HTTP error");
            bail!("command service returned HTTP {status}: {body}");
        }
        let payload: RunCommandResponse = response
            .json()
            .await
            .context("command service returned an invalid response")?;
        if !payload.success {
            let detail = payload
                .error
                .unwrap_or_else(|| "command service reported failure".into());
            tracing::error!(
                url,
                command,
                detail,
                "proxy command service rejected the command"
            );
            bail!("{detail}")
        }
        Ok(())
    }

    /// Resolve a contest player's UUID without choosing among historic names.
    pub async fn banner_uuid(&self, username: &str) -> Result<Option<String>> {
        let databases = self.databases.as_ref().context("stats database missing")?;
        let uuids = databases.uuids_for_player_name(username).await?;
        if uuids.len() != 1 {
            return Ok(None);
        }
        Ok(Some(uuid::Uuid::parse_str(&uuids[0])?.to_string()))
    }

    /// Grant a contest prize once. Callers journal the attempt before invoking this.
    pub async fn grant_banner_prize(&self, uuid: &str, group: &str) -> Result<()> {
        self.banner_prize_command(uuid, group, "addtemp", "1mo")
            .await
    }

    pub(crate) async fn banner_prize_command(
        &self,
        uuid: &str,
        group: &str,
        action: &str,
        duration: &str,
    ) -> Result<()> {
        if !matches!(
            (action, duration),
            ("addtemp", "1mo" | "1m") | ("removetemp", "1m")
        ) {
            bail!("invalid prize operation");
        }
        let uuid = uuid::Uuid::parse_str(uuid).context("invalid player UUID")?;
        if !matches!(group, "primeultra" | "eliteultra" | "legend") {
            bail!("invalid banner prize group");
        }
        let base = self
            .environment
            .proxy_command_base_url
            .as_deref()
            .context("proxy command service URL missing")?;
        let token = self
            .environment
            .proxy_command_access_token
            .as_deref()
            .context("proxy command service token missing")?;
        let command = format!("lpv user {uuid} parent {action} {group} {duration}")
            .trim_end()
            .to_owned();
        let response: RunCommandResponse = self
            .http
            .post(format!("{}/run-command", base.trim_end_matches('/')))
            .header(reqwest::header::AUTHORIZATION, token)
            .json(&RunCommandRequest { command: &command })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if !response.success {
            bail!("temporary rank command failed");
        }
        Ok(())
    }

    /// `LuckPerms` persists asynchronously. Never dispatch the grant again during verification.
    pub async fn verify_banner_prize(&self, username: &str, group: &str) -> bool {
        self.verify_banner_prize_presence(username, group, true)
            .await
    }

    pub(crate) async fn verify_banner_prize_presence(
        &self,
        username: &str,
        group: &str,
        present: bool,
    ) -> bool {
        self.verify_banner_prize_presence_checked(username, group, present, None)
            .await
    }

    pub(crate) async fn verify_banner_prize_presence_checked(
        &self,
        username: &str,
        group: &str,
        present: bool,
        expected_uuid: Option<&str>,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            if let Ok(Ok(Some(ranks))) =
                tokio::time::timeout(remaining, self.ranks_checked(username, expected_uuid)).await
                && ranks.iter().any(|r| r.eq_ignore_ascii_case(group)) == present
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(
                Duration::from_secs(2)
                    .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
            )
            .await;
        }
    }

    pub(crate) async fn selftest_account_safety(&self, username: &str, uuid: &str) -> Result<()> {
        let databases = self
            .databases
            .as_ref()
            .context("link database missing; selftest refused")?;
        if databases.mapping_for_uuid(uuid).await?.is_some() {
            bail!("BannerSelftest is linked to Discord; selftest refused");
        }
        let check = async {
            let base = self
                .environment
                .proxy_command_base_url
                .as_deref()
                .context("proxy URL missing")?;
            let token = self
                .environment
                .proxy_command_access_token
                .as_deref()
                .context("proxy token missing")?;
            let response: OnlinePlayersResponse = self
                .http
                .get(format!("{}/players", base.trim_end_matches('/')))
                .header(reqwest::header::AUTHORIZATION, token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if !response.success || response.player_count != response.players.len() {
                bail!("unsuccessful or incomplete player list");
            }
            let expected = uuid::Uuid::parse_str(uuid)?;
            for player in response.players {
                if uuid::Uuid::parse_str(&player.uuid)? == expected
                    || player.username.eq_ignore_ascii_case(username)
                {
                    return Ok::<_, anyhow::Error>(false);
                }
            }
            Ok::<_, anyhow::Error>(true)
        };
        match tokio::time::timeout(Duration::from_secs(10), check).await {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => bail!("BannerSelftest is online; selftest refused"),
            _ => bail!("Offline player check unavailable; selftest refused"),
        }
    }

    pub async fn player_for_discord(&self, discord_id: u64) -> Result<Option<(String, UserInfo)>> {
        let Some(databases) = &self.databases else {
            return Ok(None);
        };
        let Some(mapping) = databases
            .mapping_for_discord(&discord_id.to_string())
            .await?
        else {
            return Ok(None);
        };
        let Some(player) = databases.player_info(&mapping.uuid).await? else {
            return Ok(None);
        };
        let Some(top_rank) = self.top_rank(&player.name).await? else {
            return Ok(None);
        };
        let first_join_year = year_from_epoch_millis(player.first_join_millis)?;
        Ok(Some((
            player.name,
            UserInfo {
                top_rank,
                first_join_year,
            },
        )))
    }

    pub async fn user_info(&self, uuid: &str) -> Result<Option<UserInfo>> {
        let Some(databases) = &self.databases else {
            return Ok(None);
        };
        let Some(player) = databases.player_info(uuid).await? else {
            return Ok(None);
        };
        let Some(top_rank) = self.top_rank(&player.name).await? else {
            return Ok(None);
        };
        Ok(Some(UserInfo {
            top_rank,
            first_join_year: year_from_epoch_millis(player.first_join_millis)?,
        }))
    }

    pub async fn top_rank(&self, username: &str) -> Result<Option<String>> {
        Ok(self
            .ranks(username)
            .await?
            .map(|ranks| highest_rank(&ranks).to_owned()))
    }

    /// Full group list, including plus and Ultra groups, for contest eligibility.
    pub async fn ranks(&self, username: &str) -> Result<Option<Vec<String>>> {
        self.ranks_checked(username, None).await
    }

    pub(crate) async fn selftest_ranks(
        &self,
        username: &str,
        uuid: &str,
    ) -> Result<Option<Vec<String>>> {
        self.ranks_checked(username, Some(uuid)).await
    }

    async fn ranks_checked(
        &self,
        username: &str,
        expected_uuid: Option<&str>,
    ) -> Result<Option<Vec<String>>> {
        {
            let circuit = self.rank_circuit.lock().await;
            if circuit
                .open_until
                .is_some_and(|until| until > std::time::Instant::now())
            {
                bail!("rank command service circuit breaker is open");
            }
        }
        let base_url = self
            .environment
            .rank_service_base_url
            .as_deref()
            .context("rank command service URL is not configured")?;
        let token = self
            .environment
            .rank_service_access_token
            .as_deref()
            .context("rank command service token is not configured")?;
        let mut last_error = None;
        for attempt in 1..=2 {
            let response = self
                .http
                .post(format!("{}/get-ranks", base_url.trim_end_matches('/')))
                .header(reqwest::header::AUTHORIZATION, token)
                .json(&RankRequest { username })
                .timeout(Duration::from_secs(10))
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    let response: RankResponse =
                        response.json().await.context("invalid rank response")?;
                    self.reset_circuit().await;
                    if !response.success {
                        bail!(
                            "{}",
                            response
                                .error
                                .unwrap_or_else(|| "rank request failed".into())
                        )
                    }
                    if response.user_not_found {
                        return Ok(None);
                    }
                    if let Some(expected) = expected_uuid {
                        let actual = response.uuid.as_deref().context(
                            "Rank service provides no UUID identity proof; selftest refused",
                        )?;
                        if uuid::Uuid::parse_str(actual)? != uuid::Uuid::parse_str(expected)? {
                            bail!("Rank service UUID differs from command UUID; selftest refused");
                        }
                    }
                    return Ok(Some(response.ranks));
                }
                Ok(response)
                    if response.status().as_u16() != 429
                        && !response.status().is_server_error() =>
                {
                    bail!("rank command service returned HTTP {}", response.status());
                }
                Ok(response) => {
                    last_error = Some(anyhow::anyhow!(
                        "rank service returned HTTP {}",
                        response.status()
                    ));
                }
                Err(error) => last_error = Some(error.into()),
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(250 * attempt)).await;
            }
        }
        self.record_failure().await;
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("rank request failed")))
    }

    async fn record_failure(&self) {
        let mut circuit = self.rank_circuit.lock().await;
        circuit.failures = circuit.failures.saturating_add(1);
        if circuit.failures >= RANK_FAILURE_THRESHOLD {
            circuit.open_until = Some(std::time::Instant::now() + Duration::from_mins(1));
        }
    }

    async fn reset_circuit(&self) {
        *self.rank_circuit.lock().await = CircuitBreaker::default();
    }

    pub async fn hytale_data(&self) -> Result<HytaleData> {
        let endpoint = self
            .environment
            .hytale_endpoint_url
            .as_deref()
            .context("HYTALE_QUERY_ENDPOINT_URL is not configured")?;
        let username = self
            .environment
            .hytale_username
            .as_deref()
            .context("HYTALE_QUERY_USERNAME is not configured")?;
        let password = self
            .environment
            .hytale_password
            .as_deref()
            .context("HYTALE_QUERY_PASSWORD is not configured")?;
        let metrics_url = hytale_metrics_url(endpoint)?;
        let (query, metrics) = tokio::join!(
            self.http
                .get(endpoint)
                .basic_auth(username, Some(password))
                .timeout(Duration::from_secs(5))
                .send(),
            self.http
                .get(metrics_url)
                .basic_auth(username, Some(password))
                .timeout(Duration::from_secs(5))
                .send(),
        );
        let query = query
            .context("Hytale query failed")?
            .error_for_status()
            .context("Hytale query returned an error")?;
        let response: HytaleResponse = query.json().await.context("invalid Hytale response")?;
        let metrics = match metrics {
            Ok(response) if response.status().is_success() => {
                response.text().await.ok().map(|text| HytaleMetrics {
                    tps: metric_average(&text, "hytale_world_tps_avg"),
                    entities: metric_sum(&text, "hytale_entities_active"),
                    chunks: metric_sum(&text, "hytale_chunks_active"),
                })
            }
            _ => None,
        };
        Ok(HytaleData {
            player_count: response.universe.current_players,
            max_players: response.server.max_players,
            players: response.players,
            metrics,
        })
    }
}

#[derive(Deserialize)]
struct OnlinePlayersResponse {
    success: bool,
    #[serde(rename = "player-count")]
    player_count: usize,
    players: Vec<OnlinePlayer>,
}
#[derive(Deserialize)]
struct OnlinePlayer {
    uuid: String,
    username: String,
}
#[derive(Deserialize)]
struct PlayersResponse {
    success: bool,
    #[serde(rename = "player-count")]
    player_count: u64,
    #[serde(default, rename = "bot-count")]
    bot_count: Option<u64>,
}
#[derive(Deserialize)]
struct UptimeResponse {
    statistics: UptimeStatistics,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UptimeStatistics {
    server_start_unix: Option<i64>,
    current_uptime_hours: Option<f64>,
}
#[derive(Serialize)]
struct RankRequest<'a> {
    username: &'a str,
}
#[derive(Serialize)]
struct RunCommandRequest<'a> {
    command: &'a str,
}
#[derive(Deserialize)]
struct RunCommandResponse {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}
#[derive(Deserialize)]
struct RankResponse {
    #[serde(default)]
    uuid: Option<String>,
    success: bool,
    #[serde(default, rename = "user-not-found")]
    user_not_found: bool,
    #[serde(default)]
    ranks: Vec<String>,
    error: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HytaleResponse {
    server: HytaleServer,
    universe: HytaleUniverse,
    players: Vec<HytalePlayer>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HytaleServer {
    max_players: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HytaleUniverse {
    current_players: u64,
}

pub fn format_duration(total_seconds: i64) -> String {
    let values = [
        (total_seconds / 86_400, "d"),
        ((total_seconds % 86_400) / 3_600, "h"),
        ((total_seconds % 3_600) / 60, "m"),
        (total_seconds % 60, "s"),
    ];
    let result = values
        .into_iter()
        .filter(|(value, _)| *value > 0)
        .map(|(value, suffix)| format!("{value}{suffix}"))
        .collect::<Vec<_>>()
        .join(" ");
    if result.is_empty() {
        "0s".into()
    } else {
        result
    }
}

fn highest_rank(ranks: &[String]) -> &'static str {
    [
        "legend",
        "apex",
        "eliteultra",
        "elite",
        "primeultra",
        "prime",
    ]
    .into_iter()
    .find(|rank| ranks.iter().any(|candidate| candidate == rank))
    .unwrap_or("default")
}

fn year_from_epoch_millis(timestamp: i64) -> Result<i32> {
    chrono::DateTime::from_timestamp_millis(timestamp)
        .map(|date| date.year())
        .with_context(|| format!("invalid first_join timestamp: {timestamp}"))
}

fn metric_values(text: &str, name: &str) -> Vec<f64> {
    text.lines()
        .filter_map(|line| {
            let (metric, value) = line.split_once(' ')?;
            (metric == name || metric.starts_with(&format!("{name}{{")))
                .then(|| value.parse().ok())
                .flatten()
        })
        .collect()
}
fn metric_sum(text: &str, name: &str) -> Option<f64> {
    let values = metric_values(text, name);
    (!values.is_empty()).then(|| values.iter().sum())
}
fn metric_average(text: &str, name: &str) -> Option<f64> {
    let values = metric_values(text, name);
    (!values.is_empty()).then(|| {
        let count = u32::try_from(values.len()).unwrap_or(u32::MAX);
        values.iter().sum::<f64>() / f64::from(count)
    })
}

fn hytale_metrics_url(endpoint: &str) -> Result<reqwest::Url> {
    reqwest::Url::parse(endpoint)
        .context("HYTALE_QUERY_ENDPOINT_URL is invalid")?
        .join("/ApexHosting/PrometheusExporter/metrics")
        .context("failed to build the Hytale metrics URL")
}

#[cfg(test)]
mod tests {
    use super::{
        PlayerCounts, PlayersResponse, format_duration, highest_rank, hytale_metrics_url,
        metric_average, metric_sum, year_from_epoch_millis,
    };
    #[test]
    fn network_players_split_out_player_made_bots() {
        let response: PlayersResponse = serde_json::from_str(
            r#"{"success":true,"player-count":559,"human-count":173,"bot-count":386}"#,
        )
        .expect("valid response");
        assert_eq!(
            PlayerCounts::from_response(response.player_count, response.bot_count),
            PlayerCounts {
                total: 559,
                humans: 173,
                bots: 386,
            }
        );
    }
    #[test]
    fn network_players_without_bot_count_are_all_players() {
        let response: PlayersResponse =
            serde_json::from_str(r#"{"success":true,"player-count":559}"#).expect("valid response");
        assert_eq!(
            PlayerCounts::from_response(response.player_count, response.bot_count),
            PlayerCounts {
                total: 559,
                humans: 559,
                bots: 0,
            }
        );
    }
    #[test]
    fn bots_never_exceed_the_total() {
        assert_eq!(
            PlayerCounts::from_response(5, Some(9)),
            PlayerCounts {
                total: 5,
                humans: 0,
                bots: 5,
            }
        );
    }
    #[test]
    fn duration_formats_nonzero_units() {
        assert_eq!(format_duration(90_061), "1d 1h 1m 1s");
        assert_eq!(format_duration(0), "0s");
    }
    #[test]
    fn ranks_follow_role_priority() {
        assert_eq!(highest_rank(&["prime".into(), "apex".into()]), "apex");
        assert_eq!(highest_rank(&[]), "default");
    }
    #[test]
    fn first_join_year_uses_epoch_milliseconds() {
        assert_eq!(
            year_from_epoch_millis(1_786_118_048_395).expect("valid timestamp"),
            2026
        );
        assert!(year_from_epoch_millis(i64::MAX).is_err());
    }
    #[test]
    fn prometheus_metrics_are_aggregated() {
        let data = "metric{x=\"a\"} 2\nmetric{x=\"b\"} 4\n";
        assert_eq!(metric_sum(data, "metric"), Some(6.0));
        assert_eq!(metric_average(data, "metric"), Some(3.0));
    }
    #[test]
    fn hytale_metrics_use_the_endpoint_origin() {
        assert_eq!(
            hytale_metrics_url("https://example.com/query/status")
                .expect("valid metrics URL")
                .as_str(),
            "https://example.com/ApexHosting/PrometheusExporter/metrics"
        );
    }
}
