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
    pub player_count: u64,
    pub version: Option<String>,
    pub server_start_unix: Option<i64>,
    pub current_uptime_hours: Option<f64>,
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
        let (players, version, uptime) = tokio::join!(
            self.player_data(),
            self.http.get(format!("{SERVER_API}/version")).send(),
            self.http.get(format!("{SERVER_API}/uptime")).send(),
        );
        let player_count = players?;
        let version = match version {
            Ok(response) if response.status().is_success() => response
                .json::<VersionResponse>()
                .await
                .ok()
                .map(|value| value.version),
            _ => None,
        };
        let uptime = match uptime {
            Ok(response) if response.status().is_success() => response
                .json::<UptimeResponse>()
                .await
                .ok()
                .map(|value| value.statistics),
            _ => None,
        };
        Ok(ServerData {
            player_count,
            version,
            server_start_unix: uptime.as_ref().and_then(|value| value.server_start_unix),
            current_uptime_hours: uptime.and_then(|value| value.current_uptime_hours),
        })
    }

    async fn player_data(&self) -> Result<u64> {
        let base_url = std::env::var("HTTP_PROXY_COMMAND_SERVICE_BASE_URL")
            .context("HTTP_PROXY_COMMAND_SERVICE_BASE_URL is required")?;
        let token = std::env::var("HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN")
            .context("HTTP_PROXY_COMMAND_SERVICE_ACCESS_TOKEN is required")?;
        let response = self
            .http
            .get(format!("{}/players", base_url.trim_end_matches('/')))
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
        Ok(response.player_count)
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
        Ok(Some((
            player.name,
            UserInfo {
                top_rank,
                first_join_year: player.first_join.year(),
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
            first_join_year: player.first_join.year(),
        }))
    }

    pub async fn top_rank(&self, username: &str) -> Result<Option<String>> {
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
                    return Ok(Some(highest_rank(&response.ranks).to_owned()));
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
struct PlayersResponse {
    success: bool,
    #[serde(rename = "player-count")]
    player_count: u64,
}
#[derive(Deserialize)]
struct VersionResponse {
    version: String,
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
#[derive(Deserialize)]
struct RankResponse {
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
    use super::{format_duration, highest_rank, hytale_metrics_url, metric_average, metric_sum};
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
