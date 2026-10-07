//! Monthly banner contests. Public posts, guild images and prizes use an at-most-once
//! journal. Private/repeatable effects use bounded persistent retries with backoff.
mod commands;
mod gateway;
mod image;
mod model;
mod selftest;
#[cfg(test)]
mod tests;

use crate::{
    config::{self, Environment},
    server::ServerService,
};
use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{Datelike as _, Utc};
pub use commands::{bannercontest, bannerthemes};
use model::{Contest, Entry, Schedule, parse_application, prize};
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub const ANNOUNCEMENTS: u64 = 1_270_008_740_707_307_655;
pub const REVIEWS: u64 = 1_557_369_850_593_026_121;
const GENERAL: u64 = 982_192_297_645_056_040;
const EVENT_ROLE: u64 = 1_155_462_541_871_415_326;
const CLOSED: &str = "Submissions for this month are closed.";
const BAD_IMAGE: &str = "Please upload a PNG, JPG or WEBP screenshot of at least 1280×720.";
const BAD_RANK: &str = "This username can't take part. It needs Prime, Prime+, Elite, Elite+ or Apex (not an Ultra or Legend rank). Use a different username.";
const UNKNOWN: &str = "We couldn't find this username on 6b6t. Check the spelling.";
const NONE: &str =
    "No screenshots were accepted this month, so the banner stays the same. Try again next month!";

#[derive(Clone)]
pub struct BannerService {
    pool: MySqlPool,
    http: reqwest::Client,
    token: Arc<String>,
    discord_api: String,
    lock: Arc<Mutex<()>>,
}

pub async fn ensure_schema(pool: &MySqlPool) -> Result<()> {
    for statement in model::SCHEMA {
        sqlx::query(*statement).execute(pool).await?;
    }
    sqlx::query("INSERT IGNORE INTO banner_themes (year, month, theme) VALUES (2026, 10, 'Halloween'), (2026, 12, 'Christmas')").execute(pool).await?;
    Ok(())
}

impl BannerService {
    pub(crate) async fn initialize(
        pool: MySqlPool,
        http: reqwest::Client,
        environment: &Environment,
    ) -> Result<Self> {
        ensure_schema(&pool).await?;
        Ok(Self::new(pool, http, environment))
    }

    pub fn new(pool: MySqlPool, http: reqwest::Client, environment: &Environment) -> Self {
        Self {
            pool,
            http,
            token: Arc::new(
                environment
                    .discord_token
                    .trim()
                    .trim_start_matches("Bot ")
                    .to_owned(),
            ),
            discord_api: "https://discord.com/api/v10".to_owned(),
            lock: Arc::default(),
        }
    }

    pub fn start(&self, server: ServerService) {
        let service = self.clone();
        let gateway_service = self.clone();
        let gateway_server = server.clone();
        tokio::spawn(async move {
            gateway_service.gateway_loop(gateway_server).await;
        });
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                if service.poll(&server).await.is_err() {
                    tracing::error!(
                        "banner contest worker failed; persisted state will be checked next tick"
                    );
                }
            }
        });
    }

    // Advisory lock serializes workers and interaction handlers across bot instances.
    // A dedicated connection holds it while journal writes use the pool. Always release.
    async fn acquire(&self) -> Result<Option<sqlx::pool::PoolConnection<sqlx::MySql>>> {
        let mut connection = self.pool.acquire().await?;
        connection.close_on_drop();
        let acquired: i64 = sqlx::query_scalar("SELECT GET_LOCK('6b6t_banner_contest', 0)")
            .fetch_one(&mut *connection)
            .await?;
        if acquired == 1 {
            Ok(Some(connection))
        } else {
            Ok(None)
        }
    }
    async fn release(mut connection: sqlx::pool::PoolConnection<sqlx::MySql>) {
        // Close on failure so a pooled connection can never retain the named lock.
        if sqlx::query("DO RELEASE_LOCK('6b6t_banner_contest')")
            .execute(&mut *connection)
            .await
            .is_err()
        {
            let _ = connection.close().await;
        }
    }

    async fn contest(&self, id: u64) -> Result<Contest> {
        Ok(sqlx::query_as("SELECT * FROM banner_contests WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?)
    }
    async fn entries(&self, id: u64) -> Result<Vec<Entry>> {
        Ok(sqlx::query_as(
            "SELECT id,contest_id,discord_id,username,uuid,email,status,decider,reason,submitted_at,shuffle_key,review_message_id,vote_message_id,votes,revision,review_revision,review_closed FROM banner_submissions WHERE contest_id = ? ORDER BY submitted_at, id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?)
    }
    async fn theme(&self, year: i32, month: u32) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT theme FROM banner_themes WHERE year = ? AND month = ?")
                .bind(year)
                .bind(month)
                .fetch_optional(&self.pool)
                .await?
                .filter(|s: &String| !s.is_empty()),
        )
    }

    async fn insert_contest(
        &self,
        year: i32,
        month: u32,
        schedule: Schedule,
        dry: bool,
    ) -> Result<u64> {
        let key = if dry {
            format!("test-{}", uuid::Uuid::new_v4())
        } else {
            format!("{year:04}-{month:02}")
        };
        let result = sqlx::query("INSERT IGNORE INTO banner_contests (contest_key, year, month, call_at, close_at, voting_at, end_at, dry_run, theme) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&key).bind(year).bind(month).bind(schedule.call).bind(schedule.close).bind(schedule.voting).bind(schedule.end).bind(dry).bind(self.theme(year, month).await?).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            bail!("a contest for this month already exists");
        }
        Ok(result.last_insert_id())
    }

    async fn generate(&self) -> Result<()> {
        let started: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM banner_contests WHERE dry_run=FALSE AND call_message_id IS NOT NULL").fetch_one(&self.pool).await?;
        if started == 0 {
            return Ok(());
        }
        let now = Utc::now().with_timezone(&chrono_tz::Europe::Warsaw);
        let (year, month) = if now.month() == 12 {
            (now.year() + 1, 1)
        } else {
            (now.year(), now.month() + 1)
        };
        if (year, month) < (2026, 11) {
            return Ok(());
        }
        let key = format!("{year:04}-{month:02}");
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM banner_contests WHERE contest_key = ?")
                .bind(key)
                .fetch_one(&self.pool)
                .await?;
        if exists == 0 {
            self.insert_contest(year, month, Schedule::monthly(year, month)?, false)
                .await?;
        }
        Ok(())
    }

    async fn poll(&self, server: &ServerService) -> Result<()> {
        let _guard = self.lock.lock().await;
        let Some(connection) = self.acquire().await? else {
            return Ok(());
        };
        let result = self.poll_locked(server).await;
        Self::release(connection).await;
        result
    }
    async fn poll_locked(&self, server: &ServerService) -> Result<()> {
        self.flush_reports().await?;
        self.generate().await?;
        // Finished public phases must not discard a retrying private notice/card edit.
        let terminal: Vec<Contest> = sqlx::query_as("SELECT c.* FROM banner_contests c WHERE c.state IN ('complete','skipped','paused','failed') AND (c.effects LIKE '%\"state\":\"pending\"%' OR c.effects LIKE '%\"state\":\"attempted\"%' OR EXISTS (SELECT 1 FROM banner_submissions e WHERE e.contest_id=c.id AND (e.review_message_id IS NULL OR e.review_revision<>e.revision OR e.review_closed=FALSE OR (e.status<>'pending' AND COALESCE(JSON_UNQUOTE(JSON_EXTRACT(c.effects,CONCAT('$.notify_',e.id,'_',e.revision,'.state'))),'')<>'done'))))").fetch_all(&self.pool).await?;
        for contest in terminal {
            self.reconcile_entries(&contest).await?;
        }
        let contests: Vec<Contest> = sqlx::query_as("SELECT * FROM banner_contests WHERE state NOT IN ('complete', 'skipped', 'failed', 'paused') ORDER BY call_at, id").fetch_all(&self.pool).await?;
        for mut contest in contests {
            if matches!(contest.state.as_str(), "open" | "review" | "voting") {
                self.reconcile_entries(&contest).await?;
            }
            // Catch up all due phases in order, including rows inserted by SQL.
            loop {
                let now = Utc::now().timestamp();
                if !contest.dry_run {
                    let stale = match contest.state.as_str() {
                        "scheduled" => now >= contest.close_at,
                        "review" => now >= contest.voting_at && contest.end_at - now < 12 * 3600,
                        "voting" if now >= contest.end_at => {
                            let effects = self.effects(contest.id).await?;
                            effects["voting_public_at"]["result"]
                                .as_i64()
                                .is_none_or(|at| contest.end_at - at < 24 * 3600)
                        }
                        _ => false,
                    };
                    if stale {
                        sqlx::query("UPDATE banner_submissions SET status='expired',revision=revision+1 WHERE contest_id=? AND status='pending'").bind(contest.id).execute(&self.pool).await?;
                        self.queue_report(contest.id, "stale", &format!("Contest {} skipped: missed safe submission/voting window. No public catch-up post or prize. Check the dates before scheduling a new contest.", contest.id)).await?;
                        self.set_state(contest.id, "skipped").await?;
                        break;
                    }
                }
                let result = match contest.state.as_str() {
                    "scheduled" if now >= contest.call_at => self.call(&contest).await,
                    "scheduled"
                        if now >= model::theme_reminder_at(contest.call_at)?
                            && !contest.reminded =>
                    {
                        self.remind(&contest).await
                    }
                    "open" if now >= contest.close_at => self.close(&contest).await,
                    "review" if now >= contest.voting_at => self.voting(&contest).await,
                    "voting" if now >= contest.end_at => self.finish(&contest, server).await,
                    _ => break,
                };
                if result.is_err() {
                    // Retryable errors retain the phase. Public effects are held by their journal.
                    let effects = self.effects(contest.id).await?;
                    if effects.as_object().is_some_and(|e| {
                        e.iter()
                            .any(|(k, v)| !retryable(k) && v["state"] == "attempted")
                    }) {
                        self.fail(contest.id, &format!("Contest {} paused at {}: reconcile the public effects journal before resuming.", contest.id, contest.state)).await?;
                    }
                    break;
                }
                contest = self.contest(contest.id).await?;
            }
        }
        self.flush_reports().await?;
        Ok(())
    }
    async fn reconcile_entries(&self, contest: &Contest) -> Result<()> {
        if Utc::now().timestamp() >= contest.close_at
            || !matches!(contest.state.as_str(), "scheduled" | "open")
        {
            let _ = self.disable_apply(contest).await;
        }
        for entry in self.entries(contest.id).await? {
            // A private effect failure cannot block a different entry or the phase transition.
            let _ = self.reconcile_entry(contest, &entry).await;
        }
        Ok(())
    }
    async fn reconcile_entry(&self, contest: &Contest, entry: &Entry) -> Result<()> {
        if entry.review_message_id.is_none() {
            let key = format!("review_{}", entry.id);
            let id = if let Some(result) = self
                .begin(
                    contest.id,
                    &key,
                    "Upload the missing anonymous review card manually.",
                )
                .await?
            {
                result
                    .as_str()
                    .context("invalid review message ID")?
                    .to_owned()
            } else {
                let image = self.entry_image(entry.id).await?;
                let message = self.journal_message(contest.id,&key,REVIEWS,json!({"components":model::review_components(entry.id,!model::can_review(contest,Utc::now().timestamp()))}),Some(&image)).await?;
                let id = message["id"]
                    .as_str()
                    .context("missing review message ID")?
                    .to_owned();
                self.done(contest.id, &key, json!(id)).await?;
                id
            };
            sqlx::query("UPDATE banner_submissions SET review_message_id=? WHERE id=?")
                .bind(id)
                .bind(entry.id)
                .execute(&self.pool)
                .await?;
        }
        let entry = self.entry(entry.id).await?;
        let effects = self.effects(contest.id).await?;
        for (key, effect) in effects.as_object().context("invalid effects")? {
            if key.starts_with(&format!("reset_{}_", entry.id)) && effect["state"] != "done" {
                let _ = self.reset_menu(&entry, key).await;
            }
        }
        let _ = self
            .update_review(&entry, !model::can_review(contest, Utc::now().timestamp()))
            .await;
        if contest.state == "voting"
            && let Some(message) = &entry.vote_message_id
        {
            let _ = self.add_fire(contest, entry.id, message).await;
        }
        // Decision snapshots preserve every actual revision, even two changes in one tick.
        for (key, notice) in effects.as_object().context("invalid effects")? {
            if key.starts_with(&format!("notice_{}_", entry.id)) && notice["state"] == "pending" {
                let mut snapshot = entry.clone();
                snapshot.status = notice["status"]
                    .as_str()
                    .context("invalid notice status")?
                    .to_owned();
                snapshot.reason = notice["reason"].as_str().map(str::to_owned);
                snapshot.revision = u32::try_from(
                    notice["revision"]
                        .as_u64()
                        .context("invalid notice revision")?,
                )?;
                if self.notify(contest, &snapshot).await.is_ok() {
                    self.done(contest.id, key, json!(true)).await?;
                }
            }
        }
        if entry.status != "pending" {
            let _ = self.notify(contest, &entry).await;
        }
        Ok(())
    }
    async fn set_state(&self, id: u64, state: &str) -> Result<()> {
        sqlx::query("UPDATE banner_contests SET state = ? WHERE id = ?")
            .bind(state)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // Returns a previously successful effect, or records the attempt BEFORE I/O.
    // A crash or ambiguous HTTP timeout leaves 'attempted', which requires manual reconciliation.
    async fn begin(&self, contest: u64, key: &str, manual: &str) -> Result<Option<Value>> {
        let row: String = sqlx::query_scalar("SELECT effects FROM banner_contests WHERE id = ?")
            .bind(contest)
            .fetch_one(&self.pool)
            .await?;
        let mut effects: Value = serde_json::from_str(&row)?;
        if let Some(effect) = effects.get(key) {
            if effect["state"] == "done" {
                return Ok(Some(effect["result"].clone()));
            }
            if effect["dispatch_started"] == true {
                if !key.starts_with("report_") {
                    self.queue_report(contest, key, &format!("Contest {contest}: uncertain message `{key}`. Inspect Discord and reconcile its journal before retrying. {manual}")).await?;
                }
                bail!("message requires manual reconciliation");
            }
            if retryable(key) {
                let attempts = effect["attempts"].as_u64().unwrap_or(1);
                if attempts >= 5 {
                    if !key.starts_with("report_") {
                        self.queue_report(
                            contest,
                            key,
                            &format!(
                                "Contest {contest}: `{key}` failed after 5 attempts. {manual}"
                            ),
                        )
                        .await?;
                    }
                    bail!("retry limit reached");
                }
                if Utc::now().timestamp() < effect["retry_at"].as_i64().unwrap_or(0) {
                    bail!("retry deferred");
                }
            } else {
                self.queue_report(
                    contest,
                    key,
                    &format!("Contest {contest}: uncertain step `{key}`. By hand: {manual}"),
                )
                .await?;
                bail!("effect requires manual reconciliation");
            }
        }
        let message = effects[key]["message"].clone();
        let attempts = effects[key]["attempts"].as_u64().unwrap_or(0) + 1;
        effects[key] = json!({"state":"attempted", "manual":manual, "attempts":attempts, "retry_at":Utc::now().timestamp() + 15 * (1_i64 << attempts.min(5)), "nonce":uuid::Uuid::new_v4().simple().to_string()[..25]});
        if !message.is_null() {
            effects[key]["message"] = message;
        }
        sqlx::query("UPDATE banner_contests SET effects = ? WHERE id = ?")
            .bind(effects.to_string())
            .bind(contest)
            .execute(&self.pool)
            .await?;
        Ok(None)
    }
    async fn done(&self, contest: u64, key: &str, result: Value) -> Result<()> {
        let row: String = sqlx::query_scalar("SELECT effects FROM banner_contests WHERE id = ?")
            .bind(contest)
            .fetch_one(&self.pool)
            .await?;
        let mut effects: Value = serde_json::from_str(&row)?;
        effects[key]["state"] = json!("done");
        effects[key]["result"] = result;
        sqlx::query("UPDATE banner_contests SET effects = ? WHERE id = ?")
            .bind(effects.to_string())
            .bind(contest)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn fail(&self, id: u64, message: &str) -> Result<()> {
        self.queue_report(id, "failure", message).await?;
        self.set_state(id, "paused").await
    }
    // Operational reports share the contest journal, but never participate in a contest.
    async fn report(&self, message: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        let connection = self.acquire().await?.context("report journal busy")?;
        let result = async {
            let id = self.operational_journal("gateway-stop").await?;
            self.queue_report(id, "gateway_stop", message).await
        }
        .await;
        Self::release(connection).await;
        result
    }

    // A confirmed healthy session starts a new stop event. Preserve every prior
    // receipt and nonce; archived uncertain dispatches remain held across restart.
    async fn rearm_gateway_report(&self) -> Result<()> {
        let _guard = self.lock.lock().await;
        let connection = self.acquire().await?.context("report journal busy")?;
        let result = async {
            let id = self.operational_journal("gateway-stop").await?;
            let mut effects = self.effects(id).await?;
            if effects.get("report_gateway_stop").is_some() {
                let previous = effects
                    .as_object_mut()
                    .context("invalid effects")?
                    .remove("report_gateway_stop")
                    .context("missing report")?;
                effects[format!(
                    "report_gateway_stop_history_{}",
                    uuid::Uuid::new_v4().simple()
                )] = previous;
                sqlx::query("UPDATE banner_contests SET effects=? WHERE id=?")
                    .bind(effects.to_string())
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
            }
            Ok(())
        }
        .await;
        Self::release(connection).await;
        result
    }

    async fn operational_journal(&self, key: &str) -> Result<u64> {
        sqlx::query("INSERT IGNORE INTO banner_contests(contest_key,year,month,call_at,close_at,voting_at,end_at,dry_run,state) VALUES (?,2026,10,0,0,0,0,TRUE,'skipped')")
            .bind(key).execute(&self.pool).await?;
        Ok(
            sqlx::query_scalar("SELECT id FROM banner_contests WHERE contest_key=?")
                .bind(key)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    async fn modify_guild_images(&self, images: Value, reason: &str) -> Result<(u16, Value)> {
        self.request_with_reason(
            reqwest::Method::PATCH,
            &format!("/guilds/{}", config::GUILD_ID),
            Some(images),
            Some(reason),
        )
        .await
    }

    async fn effects(&self, id: u64) -> Result<Value> {
        let text: String = sqlx::query_scalar("SELECT effects FROM banner_contests WHERE id=?")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        Ok(serde_json::from_str(&text)?)
    }
    async fn queue_report(&self, id: u64, key: &str, message: &str) -> Result<()> {
        let mut effects = self.effects(id).await?;
        let key = format!("report_{key}");
        if effects.get(&key).is_none() {
            effects[&key] = json!({"state":"pending", "message":message});
            sqlx::query("UPDATE banner_contests SET effects=? WHERE id=?")
                .bind(effects.to_string())
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
    async fn flush_reports(&self) -> Result<()> {
        let rows: Vec<(u64, String)> =
            sqlx::query_as("SELECT id,effects FROM banner_contests WHERE effects LIKE '%report_%'")
                .fetch_all(&self.pool)
                .await?;
        for (id, text) in rows {
            let effects: Value = serde_json::from_str(&text)?;
            for (key, effect) in effects.as_object().context("invalid effects")? {
                if key.starts_with("report_")
                    && effect["state"] != "done"
                    && let Some(message) = effect["message"].as_str()
                    && self
                        .begin(id, key, "Inspect the staff report before resending.")
                        .await
                        .is_ok_and(|v| v.is_none())
                    && self
                        .journal_message(id, key, REVIEWS, json!({"content":message}), None)
                        .await
                        .is_ok()
                {
                    self.done(id, key, json!(true)).await?;
                }
            }
        }
        Ok(())
    }
    async fn entry_image(&self, id: u64) -> Result<Vec<u8>> {
        Ok(
            sqlx::query_scalar("SELECT image FROM banner_submissions WHERE id=?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<Value>,
    ) -> Result<Value> {
        Ok(self.request_with_status(method, path, payload).await?.1)
    }
    async fn request_with_status(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<Value>,
    ) -> Result<(u16, Value)> {
        self.request_with_reason(method, path, payload, None).await
    }
    async fn request_with_reason(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<Value>,
        reason: Option<&str>,
    ) -> Result<(u16, Value)> {
        for attempt in 0..4 {
            let mut request = self
                .http
                .request(method.clone(), format!("{}{path}", self.discord_api))
                .header("Authorization", format!("Bot {}", self.token));
            if let Some(reason) = reason {
                request = request.header("X-Audit-Log-Reason", reason);
            }
            if let Some(payload) = &payload {
                request = request.json(payload);
            }
            // Never propagate URLs containing interaction/webhook tokens.
            let Ok(response) = request.send().await else {
                tracing::warn!(failure_kind = "transport", "banner Discord request failed");
                return Err(anyhow::anyhow!("Discord transport failure"));
            };
            if response.status().as_u16() == 429 && attempt < 3 {
                Self::rate_limit_wait(response).await?;
                continue;
            }
            let status = response.status().as_u16();
            return Ok((status, Self::response(response).await?));
        }
        bail!("Discord rate limit exceeded")
    }
    async fn rate_limit_wait(response: reqwest::Response) -> Result<()> {
        let body: Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("invalid rate limit response"))?;
        let seconds = body["retry_after"]
            .as_f64()
            .context("missing rate limit delay")?;
        if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
            bail!("rate limit delay too long");
        }
        tokio::time::sleep(Duration::try_from_secs_f64(seconds + 0.1)?).await;
        Ok(())
    }
    async fn response(response: reqwest::Response) -> Result<Value> {
        let status = response.status();
        if !status.is_success() {
            tracing::warn!(
                http_status = status.as_u16(),
                "banner Discord request failed"
            );
            return Err(DiscordHttpError(status.as_u16()).into());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| anyhow::anyhow!("Discord response read failed"))?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).context("invalid Discord JSON")
    }
    async fn message(
        &self,
        channel: u64,
        mut payload: Value,
        image: Option<&[u8]>,
    ) -> Result<Value> {
        if payload.get("allowed_mentions").is_none() {
            payload["allowed_mentions"] = json!({"parse":[]});
        }
        let path = format!("/channels/{channel}/messages");
        if let Some(bytes) = image {
            payload["attachments"] = json!([{"id":0,"filename":"banner.jpg"}]);
            for attempt in 0..4 {
                let part = reqwest::multipart::Part::bytes(bytes.to_vec())
                    .file_name("banner.jpg")
                    .mime_str("image/jpeg")?;
                let form = reqwest::multipart::Form::new()
                    .text("payload_json", payload.to_string())
                    .part("files[0]", part);
                let response = self
                    .http
                    .post(format!("{}{path}", self.discord_api))
                    .header("Authorization", format!("Bot {}", self.token))
                    .multipart(form)
                    .send()
                    .await
                    .map_err(|_| anyhow::anyhow!("Discord upload transport failure"))?;
                // Only a definite 429 (no side effect) can be retried automatically.
                if response.status().as_u16() == 429 && attempt < 3 {
                    Self::rate_limit_wait(response).await?;
                    continue;
                }
                return Self::response(response).await;
            }
            bail!("Discord upload rate limit exceeded")
        }
        self.request(reqwest::Method::POST, &path, Some(payload))
            .await
    }
    // Lock-held helper: save uncertainty before POST, retry only inside the live nonce window.
    async fn journal_message(
        &self,
        contest: u64,
        key: &str,
        channel: u64,
        mut payload: Value,
        image: Option<&[u8]>,
    ) -> Result<Value> {
        let mut effects = self.effects(contest).await?;
        effects[key]["dispatch_started"] = json!(true);
        if effects[key]["nonce"].is_null() {
            effects[key]["nonce"] = json!(uuid::Uuid::new_v4().simple().to_string()[..25]);
        }
        sqlx::query("UPDATE banner_contests SET effects=? WHERE id=?")
            .bind(effects.to_string())
            .bind(contest)
            .execute(&self.pool)
            .await?;
        payload["nonce"] = effects[key]["nonce"].clone();
        payload["enforce_nonce"] = json!(true);
        let result = self.public_message(channel, payload, image).await;
        // An explicit client rejection has no message effect. Transport and server errors
        // remain uncertain even if the process restarts after this call.
        if result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<DiscordHttpError>())
            .is_some_and(|e| (400..500).contains(&e.0))
        {
            effects[key]["dispatch_started"] = json!(false);
            sqlx::query("UPDATE banner_contests SET effects=? WHERE id=?")
                .bind(effects.to_string())
                .bind(contest)
                .execute(&self.pool)
                .await?;
        }
        result
    }
    // Discord deduplicates this nonce for a few minutes. Restrict retries to one
    // short live attempt; a process restart still requires manual reconciliation.
    async fn public_message(
        &self,
        channel: u64,
        payload: Value,
        image: Option<&[u8]>,
    ) -> Result<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        for attempt in 0..5 {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                bail!("public message retry deadline exceeded");
            }
            let result = tokio::time::timeout(
                remaining.min(Duration::from_secs(10)),
                self.message(channel, payload.clone(), image),
            )
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("Discord transport failure")));
            match result {
                Ok(message) => return Ok(message),
                Err(error) => {
                    let transient = error
                        .downcast_ref::<DiscordHttpError>()
                        .is_some_and(|e| (500..=599).contains(&e.0))
                        || matches!(
                            error.to_string().as_str(),
                            "Discord transport failure"
                                | "Discord upload transport failure"
                                | "Discord response read failed"
                        );
                    if !transient || attempt == 4 {
                        return Err(error);
                    }
                    let wait = Duration::from_secs(1 << attempt);
                    if tokio::time::Instant::now() + wait >= deadline {
                        return Err(error);
                    }
                    tokio::time::sleep(wait).await;
                }
            }
        }
        bail!("public message retry limit reached")
    }
    async fn post_once(
        &self,
        contest: &Contest,
        key: &str,
        mut payload: Value,
        image: Option<&[u8]>,
    ) -> Result<String> {
        if let Some(value) = self.begin(contest.id, key, &format!("Inspect the {} message in <#{}>; record its ID in effects.{key}.result and mark it done, or send that missing post.", key, contest.channel())).await? {
            return Ok(value.as_str().context("invalid stored message ID")?.to_owned());
        }
        if contest.dry_run {
            payload["allowed_mentions"] = json!({"parse":[]});
        }
        let result = if retryable(key) {
            self.journal_message(contest.id, key, contest.channel(), payload, image)
                .await
        } else {
            payload["nonce"] = self.effects(contest.id).await?[key]["nonce"].clone();
            payload["enforce_nonce"] = json!(true);
            self.public_message(contest.channel(), payload, image).await
        };
        match result {
            Ok(value) => {
                let id = value["id"]
                    .as_str()
                    .context("missing message ID")?
                    .to_owned();
                self.done(contest.id, key, json!(id)).await?;
                Ok(id)
            }
            Err(error) => {
                if !retryable(key) {
                    self.queue_report(contest.id, key, &format!("Contest {}: `{key}` uncertain. Inspect <#{}> before sending it; record its ID in the journal. Error: {error}", contest.id, contest.channel())).await?;
                }
                Err(error)
            }
        }
    }
    async fn call(&self, contest: &Contest) -> Result<()> {
        let mut snapshot = contest.clone();
        snapshot.theme = self.theme(contest.year, contest.month).await?;
        sqlx::query("UPDATE banner_contests SET theme=? WHERE id=?")
            .bind(&snapshot.theme)
            .bind(contest.id)
            .execute(&self.pool)
            .await?;
        let announcement = model::call_text(&snapshot);
        let id = self.post_once(contest, "call", json!({"content":announcement,"allowed_mentions":role_mentions(),"components":[{"type":1,"components":[{"type":2,"style":1,"label":"Apply","custom_id":format!("banner:apply:{}",contest.id)}]}]}), None).await?;
        sqlx::query("UPDATE banner_contests SET state = 'open', call_message_id = ? WHERE id = ?")
            .bind(id)
            .bind(contest.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn remind(&self, contest: &Contest) -> Result<()> {
        if self.theme(contest.year, contest.month).await?.is_none() {
            let mut review_contest = contest.clone();
            review_contest.dry_run = true;
            self.post_once(&review_contest, "theme_reminder", json!({"content":format!("No theme is set for {}/{}. Set one with /bannerthemes set before the call <t:{}:F>, or the contest will allow any screenshot taken on 6b6t.",contest.year,contest.month,contest.call_at)}),None).await?;
        }
        sqlx::query("UPDATE banner_contests SET reminded = TRUE WHERE id = ?")
            .bind(contest.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn disable_apply(&self, contest: &Contest) -> Result<()> {
        if let Some(id) = &contest.call_message_id {
            if self
                .begin(
                    contest.id,
                    "disable_apply",
                    "Remove the Apply button manually.",
                )
                .await?
                .is_some()
            {
                return Ok(());
            }
            self.request(
                reqwest::Method::PATCH,
                &format!("/channels/{}/messages/{id}", contest.channel()),
                Some(json!({"allowed_mentions":{"parse":[]},"components":[]})),
            )
            .await?;
            self.done(contest.id, "disable_apply", json!(true)).await?;
        }
        Ok(())
    }
    async fn close(&self, contest: &Contest) -> Result<()> {
        let _ = self.disable_apply(contest).await;
        self.set_state(contest.id, "review").await
    }
    async fn voting(&self, contest: &Contest) -> Result<()> {
        let mut entries = self.entries(contest.id).await?;
        for entry in &entries {
            if entry.status == "pending" {
                sqlx::query("UPDATE banner_submissions SET status='expired', revision=revision+1 WHERE id=? AND status='pending'").bind(entry.id).execute(&self.pool).await?;
                let updated = self.entry(entry.id).await?;
                let _ = self.notify(contest, &updated).await;
            }
            let _ = self.update_review(&self.entry(entry.id).await?, true).await;
        }
        entries.retain(|entry| entry.status == "approved");
        if entries.is_empty() {
            self.post_once(contest, "no_entries", json!({"content":NONE}), None)
                .await?;
            return self.set_state(contest.id, "complete").await;
        }
        self.post_once(
            contest,
            "voting_header",
            json!({"content":model::voting_text(contest),"allowed_mentions":role_mentions()}),
            None,
        )
        .await?;
        // Durable randomized ordering: UUID-generated keys are stored at submission.
        entries.sort_by(|a, b| a.shuffle_key.cmp(&b.shuffle_key));
        for (index, entry) in entries.iter().enumerate() {
            let key = format!("vote_{}", entry.id);
            let id = self
                .post_once(
                    contest,
                    &key,
                    json!({"content":format!("Screenshot {}",index+1)}),
                    Some(&self.entry_image(entry.id).await?),
                )
                .await?;
            sqlx::query("UPDATE banner_submissions SET vote_message_id=? WHERE id=?")
                .bind(&id)
                .bind(entry.id)
                .execute(&self.pool)
                .await?;
            let _ = self.add_fire(contest, entry.id, &id).await;
        }
        self.done(
            contest.id,
            "voting_public_at",
            json!(Utc::now().timestamp()),
        )
        .await?;
        self.set_state(contest.id, "voting").await
    }

    async fn add_fire(&self, contest: &Contest, id: u64, message: &str) -> Result<()> {
        let key = format!("fire_{id}");
        if self
            .begin(contest.id, &key, "Add the fire reaction manually.")
            .await?
            .is_some()
        {
            return Ok(());
        }
        self.request(
            reqwest::Method::PUT,
            &format!(
                "/channels/{}/messages/{message}/reactions/%F0%9F%94%A5/@me",
                contest.channel()
            ),
            None,
        )
        .await?;
        self.done(contest.id, &key, json!(true)).await
    }
    async fn votes(&self, contest: &Contest, entry: &Entry) -> Result<u64> {
        let message = entry
            .vote_message_id
            .as_deref()
            .context("missing voting message")?;
        let mut voters = std::collections::HashSet::new();
        // Discord separates normal and burst reactions. Count each human once.
        for reaction_type in [0, 1] {
            let mut after = String::new();
            loop {
                let users = self.request(reqwest::Method::GET, &format!("/channels/{}/messages/{message}/reactions/%F0%9F%94%A5?limit=100&type={reaction_type}{after}",contest.channel()), None).await?;
                let users = users.as_array().context("invalid reaction users")?;
                for user in users {
                    if user["bot"].as_bool() != Some(true) {
                        voters.insert(
                            user["id"]
                                .as_str()
                                .context("invalid reaction user ID")?
                                .to_owned(),
                        );
                    }
                }
                if users.len() < 100 {
                    break;
                }
                let cursor = users.last().context("missing reaction page")?["id"]
                    .as_str()
                    .context("invalid user ID")?;
                let next = format!("&after={cursor}");
                if next == after {
                    bail!("reaction pagination did not advance");
                }
                after = next;
            }
        }
        Ok(u64::try_from(voters.len())?)
    }
    async fn finish(&self, contest: &Contest, server: &ServerService) -> Result<()> {
        let mut entries = self.entries(contest.id).await?;
        entries.retain(|entry| entry.status == "approved");
        for entry in &mut entries {
            let key = format!("count_{}", entry.id);
            entry.votes = if let Some(value) = self
                .begin(
                    contest.id,
                    &key,
                    "Recount the human fire reactions and store the count in the effects journal.",
                )
                .await?
            {
                value.as_u64().context("invalid stored vote count")?
            } else {
                let count = self.votes(contest, entry).await?;
                self.done(contest.id, &key, json!(count)).await?;
                count
            };
            sqlx::query("UPDATE banner_submissions SET votes=? WHERE id=?")
                .bind(entry.votes)
                .bind(entry.id)
                .execute(&self.pool)
                .await?;
        }
        entries.sort_by(model::winner_order);
        let winner = entries.first().context("no approved winner")?;
        let stored_rank: Option<String> =
            sqlx::query_scalar("SELECT prize FROM banner_winners WHERE contest_id=?")
                .bind(contest.id)
                .fetch_optional(&self.pool)
                .await?;
        let winner_posted = self.effects(contest.id).await?["winner"]["state"] == "done";
        if !winner_posted {
            let _ = self
                .begin(
                    contest.id,
                    "preflight",
                    "Recheck winner ranks and UUID before announcing.",
                )
                .await?;
        }
        let current = server
            .ranks(&winner.username)
            .await?
            .and_then(|r| prize(&r));
        let identity = server.banner_uuid(&winner.username).await?;
        let Some(rank) = (if winner_posted {
            stored_rank.as_deref().or(current)
        } else {
            current
        }) else {
            self.queue_report(contest.id, "eligibility", "Winner is no longer eligible. No winner announcement or prize; check ranks before resuming.").await?;
            self.set_state(contest.id, "paused").await?;
            return Ok(());
        };
        if !winner_posted && identity.as_deref() != Some(&winner.uuid) {
            self.queue_report(
                contest.id,
                "identity",
                "Winner UUID no longer resolves uniquely. No winner announcement or prize.",
            )
            .await?;
            self.set_state(contest.id, "paused").await?;
            return Ok(());
        }
        self.done(contest.id, "preflight", json!(true)).await?;
        let image = self.entry_image(winner.id).await?;
        let command = format!("lpv user {} parent addtemp {rank} 1mo", winner.uuid);
        // Winner is logged before Discord and console effects; no email in any outgoing text.
        let expiry = model::month_expiry(Utc::now())?.timestamp();
        sqlx::query("INSERT IGNORE INTO banner_winners (contest_id,submission_id,year,month,username,uuid,discord_id,email,prize,expires_at,image,votes,dry_run) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(contest.id).bind(winner.id).bind(contest.year).bind(contest.month).bind(&winner.username).bind(&winner.uuid).bind(&winner.discord_id).bind(&winner.email).bind(rank).bind(expiry).bind(&image).bind(winner.votes).bind(contest.dry_run).execute(&self.pool).await?;
        self.post_once(contest,"winner",json!({"content":model::winner_text(contest,winner,rank),"allowed_mentions":role_mentions()}),Some(&image)).await?;
        if contest.dry_run {
            self.post_once(contest,"dry_actions",json!({"content":format!("TEST: would set banner, splash and discovery_splash with one Modify Guild call; would run `{command}` and verify /get-ranks. No image or rank was changed.")}),None).await?;
        } else {
            if self.begin(contest.id,"guild_images","Set banner, splash and discovery_splash to the winner's banner.jpg using one PATCH /guilds/917520262797344779.").await?.is_none() {
                let uri=format!("data:image/jpeg;base64,{}",STANDARD.encode(&image));
                if self.modify_guild_images(json!({"banner":uri,"splash":uri,"discovery_splash":uri}), "banner contest winner").await.is_err() {
                    self.queue_report(contest.id, "guild_failure", "Set the winning banner.jpg manually in all three guild image slots: banner, splash and discovery_splash. The Modify Guild step failed and will not be retried.").await?;
                }
                self.done(contest.id,"guild_images",json!("attempt finished; check guild slots if reported")).await?;
            }
            if self.begin(contest.id,"prize",&format!("Check /get-ranks first; if missing run `{command}`, then confirm {rank} in /get-ranks.")).await?.is_none() {
                // Recheck immediately before granting, after the guild modification.
                let eligible = server.ranks(&winner.username).await.ok().flatten().and_then(|r|prize(&r)) == Some(rank)
                    && server.banner_uuid(&winner.username).await.ok().flatten().as_deref() == Some(&winner.uuid);
                let granted=eligible && server.grant_banner_prize(&winner.uuid,rank).await.is_ok();
                let verified=granted && server.verify_banner_prize(&winner.username, rank).await;
                if !verified { self.queue_report(contest.id, "prize_failure", &format!("Contest {}: prize not verified. By hand: check eligibility and /get-ranks, then run `{command}` only if the rank is missing; verify `{rank}` with /get-ranks. Never run addtemp twice.",contest.id)).await?; }
                self.done(contest.id,"prize",json!({"verified":verified})).await?;
            }
        }
        self.set_state(contest.id, "complete").await
    }

    async fn entry(&self, id: u64) -> Result<Entry> {
        Ok(
            sqlx::query_as("SELECT id,contest_id,discord_id,username,uuid,email,status,decider,reason,submitted_at,shuffle_key,review_message_id,vote_message_id,votes,revision,review_revision,review_closed FROM banner_submissions WHERE id=?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }
    async fn callback(&self, interaction: &Value, payload: Value) -> Result<()> {
        let id = interaction["id"]
            .as_str()
            .context("missing interaction ID")?;
        let token = interaction["token"]
            .as_str()
            .context("missing interaction token")?;
        self.request(
            reqwest::Method::POST,
            &format!("/interactions/{id}/{token}/callback"),
            Some(payload),
        )
        .await?;
        Ok(())
    }
    async fn reply(&self, interaction: &Value, text: &str) -> Result<()> {
        self.callback(
            interaction,
            json!({"type":4,"data":{"content":text,"flags":64,"allowed_mentions":{"parse":[]}}}),
        )
        .await
    }
    async fn finish_reply(&self, interaction: &Value, text: &str) -> Result<()> {
        let app = interaction["application_id"]
            .as_str()
            .context("missing application ID")?;
        let token = interaction["token"]
            .as_str()
            .context("missing interaction token")?;
        self.request(
            reqwest::Method::PATCH,
            &format!("/webhooks/{app}/{token}/messages/@original"),
            Some(json!({"content":text,"allowed_mentions":{"parse":[]}})),
        )
        .await?;
        Ok(())
    }
    async fn receive(&self, interaction: Value, server: ServerService) -> Result<()> {
        let custom = interaction["data"]["custom_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if !custom.starts_with("banner:") {
            return Ok(());
        }
        if interaction["guild_id"].as_str() != Some(&config::GUILD_ID.to_string()) {
            return Ok(());
        }
        let parts: Vec<&str> = custom.split(':').collect();
        if parts.len() != 3 {
            return Ok(());
        }
        let id: u64 = parts[2].parse()?;
        let kind = interaction["type"].as_u64().unwrap_or(0);
        if kind == 3 && parts[1] == "apply" {
            let contest = self.contest(id).await?;
            if contest.state != "open" || Utc::now().timestamp() >= contest.close_at {
                return self.reply(&interaction, CLOSED).await;
            }
            return self.callback(&interaction, model::apply_modal(id)).await;
        }
        if kind == 3 && parts[1] == "deny" {
            if !model::reviewer(&interaction) {
                return self
                    .reply(&interaction, "Only banner reviewers can decide.")
                    .await;
            }
            let entry = self.entry(id).await?;
            let contest = self.contest(entry.contest_id).await?;
            if !model::can_review(&contest, Utc::now().timestamp()) {
                return self.reply(&interaction, "Moderation is closed.").await;
            }
            let reason = interaction["data"]["values"][0]
                .as_str()
                .context("missing deny reason")?;
            if reason == "Other" {
                self.callback(&interaction,json!({"type":9,"data":{"custom_id":format!("banner:other:{id}"),"title":"Deny screenshot","components":[{"type":1,"components":[{"type":4,"custom_id":"reason","style":2,"label":"Reason","required":true,"min_length":1,"max_length":500}]}]}})).await?;
                // Persist the menu reset so a transient PATCH failure does not trap Other.
                let key = format!(
                    "reset_{}_{}",
                    entry.id,
                    interaction["id"]
                        .as_str()
                        .context("missing interaction ID")?
                );
                let _guard = self.lock.lock().await;
                let connection = loop {
                    if let Some(connection) = self.acquire().await? {
                        break connection;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                };
                let result = self.reset_menu(&entry, &key).await;
                Self::release(connection).await;
                return result;
            }
        }
        self.callback(
            &interaction,
            json!({"type":5,"data":{"flags":64,"allowed_mentions":{"parse":[]}}}),
        )
        .await?;
        let _guard = self.lock.lock().await;
        let Some(connection) = self.acquire().await? else {
            return self
                .finish_reply(&interaction, "The contest is busy. Please try again.")
                .await;
        };
        let result = match (kind, parts[1]) {
            (5, "form") => self.submit(id, &interaction, &server).await,
            (3, "approve" | "deny") | (5, "other") => self.decide(id, parts[1], &interaction).await,
            _ => Ok("Unknown banner action.".to_owned()),
        };
        Self::release(connection).await;
        let text = match result {
            Ok(text) => text,
            Err(error) => {
                // Only structured diagnostics; never user input, URLs, tokens or response bodies.
                let status = error.downcast_ref::<DiscordHttpError>().map(|e| e.0);
                tracing::error!(
                    http_status = status,
                    "banner interaction failed; inspect persisted contest state"
                );
                "We couldn't complete this step. Please ask staff in the server.".to_owned()
            }
        };
        self.finish_reply(&interaction, &text).await
    }
    async fn submit(&self, id: u64, interaction: &Value, server: &ServerService) -> Result<String> {
        let contest = self.contest(id).await?;
        if contest.state != "open" || Utc::now().timestamp() >= contest.close_at {
            return Ok(CLOSED.into());
        }
        let Ok(application) = parse_application(&interaction["data"]) else {
            return Ok(BAD_IMAGE.into());
        };
        if !model::valid_email(&application.email) {
            return Ok("Please enter a valid email address.".into());
        }
        if !model::valid_username(&application.username) {
            return Ok(UNKNOWN.into());
        }
        let user = model::user_id(interaction).context("missing user")?;
        let duplicate_user: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM banner_submissions WHERE contest_id=? AND discord_id=?",
        )
        .bind(id)
        .bind(user)
        .fetch_one(&self.pool)
        .await?;
        if duplicate_user != 0 {
            return Ok("You already sent a screenshot this month.".into());
        }
        let duplicate_name: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM banner_submissions WHERE contest_id=? AND username=?",
        )
        .bind(id)
        .bind(&application.username)
        .fetch_one(&self.pool)
        .await?;
        if duplicate_name != 0 {
            return Ok("This username already has a screenshot in this month's contest.".into());
        }
        let Some(ranks) = server.ranks(&application.username).await? else {
            return Ok(UNKNOWN.into());
        };
        let Some(rank) = prize(&ranks) else {
            return Ok(BAD_RANK.into());
        };
        let Some(uuid) = server.banner_uuid(&application.username).await? else {
            return Ok(UNKNOWN.into());
        };
        let duplicate_uuid: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM banner_submissions WHERE contest_id=? AND uuid=?",
        )
        .bind(id)
        .bind(&uuid)
        .fetch_one(&self.pool)
        .await?;
        if duplicate_uuid != 0 {
            return Ok("This username already has a screenshot in this month's contest.".into());
        }
        if application.size == 0 || application.size > image::MAX_DOWNLOAD {
            return Ok(BAD_IMAGE.into());
        }
        let Ok(bytes) = self.download(&application.url).await else {
            return Ok(BAD_IMAGE.into());
        };
        let Ok(crop) = tokio::task::spawn_blocking(move || image::crop(&bytes)).await? else {
            return Ok(BAD_IMAGE.into());
        };
        // A slow rank service/download must never admit an entry after the deadline.
        if Utc::now().timestamp() >= contest.close_at {
            return Ok(CLOSED.into());
        }
        sqlx::query("INSERT INTO banner_submissions (contest_id,discord_id,username,uuid,email,prize,image,submitted_at,shuffle_key) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(id).bind(user).bind(&application.username).bind(uuid).bind(&application.email).bind(rank).bind(&crop).bind(Utc::now().timestamp_millis()).bind(uuid::Uuid::new_v4().to_string()).execute(&self.pool).await?;
        Ok(
            "Thanks! Your screenshot is waiting for review. We'll message you when it's checked."
                .into(),
        )
    }
    async fn download(&self, url: &str) -> Result<Vec<u8>> {
        // No redirects, so a CDN URL cannot redirect the bot onto internal services.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        #[cfg(test)]
        let local_url;
        #[cfg(test)]
        let url = if self.discord_api.starts_with("http://127.0.0.1:") {
            local_url = format!("{}/attachment", self.discord_api);
            local_url.as_str()
        } else {
            url
        };
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("image download failed"))?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > image::MAX_DOWNLOAD)
        {
            bail!("image rejected");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("image read failed"))?
        {
            if bytes.len() + chunk.len() > usize::try_from(image::MAX_DOWNLOAD)? {
                bail!("image too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn decide(&self, id: u64, action: &str, interaction: &Value) -> Result<String> {
        if !model::reviewer(interaction) {
            return Ok("Only banner reviewers can decide.".into());
        }
        let entry = self.entry(id).await?;
        let contest = self.contest(entry.contest_id).await?;
        if !model::can_review(&contest, Utc::now().timestamp()) {
            return Ok("Moderation is closed.".into());
        }
        let reason = if action == "approve" {
            None
        } else if action == "other" {
            Some(
                model::field(&interaction["data"]["components"], "reason")
                    .and_then(|f| f["value"].as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
            )
        } else {
            let reason = interaction["data"]["values"][0]
                .as_str()
                .unwrap_or_default();
            if !matches!(reason, "Low quality" | "Off-topic" | "Inappropriate") {
                return Ok("Invalid deny reason.".into());
            }
            Some(reason.to_owned())
        };
        if reason
            .as_ref()
            .is_some_and(|r| r.is_empty() || r.chars().count() > 500)
        {
            return Ok("Please enter a reason of 1 to 500 characters.".into());
        }
        let status = if reason.is_none() {
            "approved"
        } else {
            "denied"
        };
        if entry.status == status && entry.reason == reason {
            return Ok("Decision saved.".into());
        }
        let mut effects = self.effects(contest.id).await?;
        let revision = entry
            .revision
            .checked_add(1)
            .context("decision revision overflow")?;
        effects[format!("notice_{id}_{revision}")] =
            json!({"state":"pending","status":status,"reason":reason,"revision":revision});
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE banner_submissions SET status=?,reason=?,decider=?,revision=? WHERE id=?",
        )
        .bind(status)
        .bind(&reason)
        .bind(model::user_id(interaction))
        .bind(revision)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE banner_contests SET effects=? WHERE id=?")
            .bind(effects.to_string())
            .bind(contest.id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok("Decision saved.".into())
    }
    async fn reset_menu(&self, entry: &Entry, key: &str) -> Result<()> {
        if self
            .begin(
                entry.contest_id,
                key,
                "Reset the denial menu on the review card.",
            )
            .await?
            .is_some()
        {
            return Ok(());
        }
        if let Some(message) = &entry.review_message_id {
            let contest = self.contest(entry.contest_id).await?;
            self.request(reqwest::Method::PATCH,&format!("/channels/{REVIEWS}/messages/{message}"),Some(json!({"components":model::review_components(entry.id,!model::can_review(&contest,Utc::now().timestamp())),"allowed_mentions":{"parse":[]}}))).await?;
        }
        self.done(entry.contest_id, key, json!(true)).await
    }
    async fn update_review(&self, entry: &Entry, disabled: bool) -> Result<()> {
        if entry.review_revision == entry.revision && entry.review_closed == disabled {
            return Ok(());
        }
        if let Some(id) = &entry.review_message_id {
            let key = format!("patch_{}_{}_{}", entry.id, entry.revision, disabled);
            if self
                .begin(entry.contest_id, &key, "Update the review card by hand.")
                .await?
                .is_some()
            {
                return Ok(());
            }
            let decision_text = entry.decider.as_ref().map_or_else(String::new, |user| {
                format!(
                    "{} by <@{user}>",
                    if entry.status == "approved" {
                        "Approved"
                    } else {
                        "Denied"
                    }
                )
            });
            self.request(reqwest::Method::PATCH,&format!("/channels/{REVIEWS}/messages/{id}"),Some(json!({"content":decision_text,"allowed_mentions":{"parse":[]},"components":model::review_components(entry.id,disabled)}))).await?;
            self.done(entry.contest_id, &key, json!(true)).await?;
            sqlx::query(
                "UPDATE banner_submissions SET review_revision=?,review_closed=? WHERE id=?",
            )
            .bind(entry.revision)
            .bind(disabled)
            .bind(entry.id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
    async fn notify(&self, contest: &Contest, entry: &Entry) -> Result<()> {
        let text=match entry.status.as_str() {
            "approved"=>format!("Your screenshot for the 6b6t Discord banner was accepted! Voting starts <t:{}:F> in <#{}>.",contest.voting_at,contest.channel()),
            "denied"=>format!("Your screenshot for the 6b6t Discord banner was denied: {}.",entry.reason.as_deref().unwrap_or("Other")),
            _=>"Your screenshot for the 6b6t Discord banner wasn't reviewed in time, so it's not in the vote. Sorry!".to_owned(),
        };
        let key = format!("notify_{}_{}", entry.id, entry.revision);
        if self.begin(contest.id,&key,"Check whether the player received the decision; send the missing notification once by hand.").await?.is_some(){return Ok(());}
        if contest.dry_run {
            // Test mode confines ALL bot posts, including decision notifications, to reviews.
            self.journal_message(
                contest.id,
                &key,
                REVIEWS,
                json!({"content":format!("TEST decision notification: {text}")}),
                None,
            )
            .await?;
        } else {
            self.deliver_notification(contest.id, &key, &entry.discord_id, &text, GENERAL)
                .await?;
        }

        self.done(contest.id, &key, json!(true)).await
    }
    async fn deliver_notification(
        &self,
        id: u64,
        key: &str,
        user: &str,
        text: &str,
        fallback: u64,
    ) -> Result<&'static str> {
        let dm = self
            .request(
                reqwest::Method::POST,
                "/users/@me/channels",
                Some(json!({"recipient_id":user})),
            )
            .await;
        let delivered = if let Ok(dm) = dm {
            if let Some(channel) = dm["id"].as_str().and_then(|id| id.parse::<u64>().ok()) {
                match self
                    .journal_message(id, key, channel, json!({"content":text}), None)
                    .await
                {
                    Ok(_) => true,
                    Err(e)
                        if e.downcast_ref::<DiscordHttpError>()
                            .is_some_and(|e| e.0 == 403) =>
                    {
                        false
                    }
                    Err(e) => return Err(e),
                }
            } else {
                bail!("missing DM channel");
            }
        } else if dm
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<DiscordHttpError>())
            .is_some_and(|e| e.0 == 403)
        {
            false
        } else {
            return Err(dm.err().context("DM request failed")?);
        };
        if !delivered {
            self.journal_message(id,key,fallback,json!({"content":format!("<@{}> {text}",user),"allowed_mentions":{"parse":[],"users":[user]}}),None).await?;
        }
        Ok(if delivered { "DM" } else { "fallback" })
    }
}
fn role_mentions() -> Value {
    json!({"parse":[],"roles":[EVENT_ROLE.to_string()]})
}

fn retryable(key: &str) -> bool {
    [
        "review_", "patch_", "notify_", "fire_", "count_", "reset_", "report_",
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix))
        || key == "theme_reminder"
        || key == "disable_apply"
        || key == "preflight"
}

#[derive(Debug)]
struct DiscordHttpError(u16);
impl std::fmt::Display for DiscordHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Discord HTTP {}", self.0)
    }
}
impl std::error::Error for DiscordHttpError {}
