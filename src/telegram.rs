use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::{sync::Mutex, task::JoinSet};

use crate::{
    config::{TelegramConfig, TelegramRoute},
    database::Databases,
};

const TEXT_LIMIT: usize = 4_096;

#[derive(Clone)]
pub struct TelegramService {
    client: TelegramClient,
    config: Arc<TelegramConfig>,
    databases: Option<Databases>,
    jobs: Arc<Mutex<()>>,
    tasks: Arc<Mutex<TaskQueue>>,
    shutting_down: Arc<AtomicBool>,
}

struct TaskQueue {
    accepting: bool,
    tasks: JoinSet<()>,
}

#[derive(Clone)]
struct TelegramClient {
    http: reqwest::Client,
    base_url: String,
    retry_attempts: usize,
    last_request: Arc<Mutex<HashMap<String, std::time::Instant>>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TelegramReference {
    message_id: i64,
    kind: TelegramMessageKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum TelegramMessageKind {
    Text,
    Photo,
    Video,
    Document,
}

#[derive(Clone, Debug)]
struct Attachment {
    filename: String,
    url: String,
    content_type: Option<String>,
}

#[derive(Clone, Debug)]
struct Payload {
    text: Vec<String>,
    attachments: Vec<Attachment>,
}

struct DeliveryFailure {
    error: anyhow::Error,
    references: Vec<TelegramReference>,
}

#[derive(Debug, sqlx::FromRow)]
struct CrosspostRow {
    content_hash: String,
    status: String,
    telegram_messages: Option<String>,
}

#[derive(Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    error_code: Option<u16>,
    parameters: Option<TelegramParameters>,
}
#[derive(Deserialize)]
struct TelegramParameters {
    retry_after: Option<u64>,
}
#[derive(Deserialize)]
struct TelegramMessage {
    message_id: i64,
}
#[derive(Deserialize)]
struct TelegramIdentity {
    id: i64,
    username: Option<String>,
}

impl TelegramService {
    pub fn new(
        http: reqwest::Client,
        config: TelegramConfig,
        databases: Option<Databases>,
    ) -> Self {
        let client = TelegramClient {
            http,
            base_url: format!("https://api.telegram.org/bot{}", config.token),
            retry_attempts: config.retry_attempts,
            last_request: Arc::new(Mutex::new(HashMap::new())),
        };
        Self {
            client,
            config: Arc::new(config),
            databases,
            jobs: Arc::new(Mutex::new(())),
            tasks: Arc::new(Mutex::new(TaskQueue {
                accepting: true,
                tasks: JoinSet::new(),
            })),
            shutting_down: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn queue_message_create(&self, message: serenity::Message) {
        let mut queue = self.tasks.lock().await;
        if !queue.accepting || self.is_shutting_down() {
            return;
        }
        reap_completed_tasks(&mut queue.tasks);
        let service = self.clone();
        queue.tasks.spawn(async move {
            if let Err(error) = service.message_create(&message).await {
                tracing::error!(%error, message_id = %message.id, "Telegram create crosspost failed");
            }
        });
    }

    pub async fn queue_message_update(&self, message: serenity::Message) {
        let mut queue = self.tasks.lock().await;
        if !queue.accepting || self.is_shutting_down() {
            return;
        }
        reap_completed_tasks(&mut queue.tasks);
        let service = self.clone();
        queue.tasks.spawn(async move {
            if let Err(error) = service.message_update(&message).await {
                tracing::error!(%error, message_id = %message.id, "Telegram update crosspost failed");
            }
        });
    }

    pub async fn queue_message_delete(
        &self,
        channel_id: serenity::ChannelId,
        message_id: serenity::MessageId,
    ) {
        let mut queue = self.tasks.lock().await;
        if !queue.accepting || self.is_shutting_down() {
            return;
        }
        reap_completed_tasks(&mut queue.tasks);
        let service = self.clone();
        queue.tasks.spawn(async move {
            if let Err(error) = service.message_delete(channel_id, message_id).await {
                tracing::error!(%error, %channel_id, %message_id, "Telegram delete crosspost failed");
            }
        });
    }

    pub async fn ready(&self, ctx: &serenity::Context) -> Result<()> {
        if self.is_shutting_down() {
            return Ok(());
        }
        let identity: TelegramIdentity = self
            .client
            .request("getMe", "connection", serde_json::json!({}))
            .await?;
        tracing::info!(
            telegram_id = identity.id,
            username = identity.username.as_deref().unwrap_or("unknown"),
            "Telegram crossposting connected"
        );
        if self.databases.is_none() {
            tracing::warn!(
                "Telegram routes are configured without MySQL; crossposting is disabled to avoid duplicate deliveries"
            );
            return Ok(());
        }
        let mut failed = false;
        for route in &self.config.routes {
            if self.is_shutting_down() {
                return Ok(());
            }
            if let Err(error) = self.initialize_route(ctx, route).await {
                failed = true;
                tracing::error!(%error, route = route.id, "failed to initialize Telegram route");
            }
        }
        if let Err(error) = self.recover(ctx).await {
            failed = true;
            tracing::error!(%error, "failed to recover Telegram crossposts");
        }
        if failed {
            anyhow::bail!("one or more Telegram routes failed to initialize")
        }
        Ok(())
    }

    pub async fn message_create(&self, message: &serenity::Message) -> Result<()> {
        let mut failed = false;
        for route in self
            .config
            .routes
            .iter()
            .filter(|route| route.discord_channel_id == message.channel_id)
        {
            if let Err(error) = self.deliver(route, message, false).await {
                failed = true;
                tracing::error!(%error, route = route.id, message_id = %message.id, "Telegram route delivery failed");
            }
        }
        if failed {
            anyhow::bail!("one or more Telegram routes failed to deliver the message")
        }
        Ok(())
    }
    pub async fn message_update(&self, message: &serenity::Message) -> Result<()> {
        if self.config.sync_edits {
            let mut failed = false;
            for route in self
                .config
                .routes
                .iter()
                .filter(|route| route.discord_channel_id == message.channel_id)
            {
                if let Err(error) = self.deliver(route, message, true).await {
                    failed = true;
                    tracing::error!(%error, route = route.id, message_id = %message.id, "Telegram route update failed");
                }
            }
            if failed {
                anyhow::bail!("one or more Telegram routes failed to update the message")
            }
        }
        Ok(())
    }
    pub async fn message_delete(
        &self,
        channel_id: serenity::ChannelId,
        message_id: serenity::MessageId,
    ) -> Result<()> {
        if !self.config.sync_deletes {
            return Ok(());
        }
        let Some(database) = &self.databases else {
            return Ok(());
        };
        let _guard = self.jobs.lock().await;
        let mut failed = false;
        for route in self
            .config
            .routes
            .iter()
            .filter(|route| route.discord_channel_id == channel_id)
        {
            let result: Result<()> = async {
                let Some(row) = self.crosspost(route, message_id).await? else {
                    return Ok(());
                };
                let references = parse_references(row.telegram_messages.as_deref());
                self.client
                    .delete(&route.telegram_chat_id, &references)
                    .await?;
                sqlx::query("UPDATE telegram_crossposts SET status = 'deleted', last_error = NULL WHERE route_id = ? AND discord_message_id = ?")
                    .bind(&route.id).bind(message_id.to_string()).execute(&database.link).await?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                failed = true;
                tracing::error!(%error, route = route.id, %message_id, "Telegram route deletion failed");
            }
        }
        if failed {
            anyhow::bail!("one or more Telegram routes failed to delete the message")
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::Release);
        let mut queue = self.tasks.lock().await;
        queue.accepting = false;
        while let Some(result) = queue.tasks.join_next().await {
            if let Err(error) = result {
                tracing::error!(%error, "Telegram delivery task failed while shutting down");
            }
        }
        drop(queue);
        let _guard = self.jobs.lock().await;
        tracing::info!("Telegram delivery queue drained");
    }

    fn is_shutting_down(&self) -> bool {
        self.shutting_down
            .load(std::sync::atomic::Ordering::Acquire)
    }

    async fn deliver(
        &self,
        route: &TelegramRoute,
        message: &serenity::Message,
        is_update: bool,
    ) -> Result<()> {
        if message.author.id.get() == crate::config::APPLICATION_ID {
            return Ok(());
        }
        let Some(database) = &self.databases else {
            return Ok(());
        };
        let _guard = self.jobs.lock().await;
        let payload = build_payload(route, message);
        let hash = payload_hash(&payload);
        let existing = self.crosspost(route, message.id).await?;
        if existing
            .as_ref()
            .is_some_and(|row| matches!(row.status.as_str(), "deleted" | "ignored"))
        {
            return Ok(());
        }
        if existing
            .as_ref()
            .is_some_and(|row| row.status == "sent" && row.content_hash == hash)
        {
            return Ok(());
        }
        if is_update && existing.as_ref().is_none_or(|row| row.status != "sent") {
            let checkpoint = self.route_checkpoint(route).await?;
            if !is_after_checkpoint(message.id, checkpoint) {
                if existing.is_some() {
                    sqlx::query("UPDATE telegram_crossposts SET status = 'ignored', last_error = NULL WHERE route_id = ? AND discord_message_id = ?")
                        .bind(&route.id)
                        .bind(message.id.to_string())
                        .execute(&database.link)
                        .await?;
                }
                tracing::info!(route = route.id, message_id = %message.id, ?checkpoint, "ignored Telegram update for a message at or before the route checkpoint");
                return Ok(());
            }
        }
        if is_update && existing.is_none() {
            tracing::info!(route = route.id, message_id = %message.id, "Telegram update arrived before create; delivering it as new");
        }
        let old_references = existing
            .as_ref()
            .map(|row| parse_references(row.telegram_messages.as_deref()))
            .unwrap_or_default();
        sqlx::query("INSERT INTO telegram_crossposts (route_id, discord_message_id, discord_channel_id, telegram_chat_id, content_hash, status, telegram_messages, attempt_count, last_error) VALUES (?, ?, ?, ?, ?, 'pending', NULL, 1, NULL) ON DUPLICATE KEY UPDATE discord_channel_id = VALUES(discord_channel_id), telegram_chat_id = VALUES(telegram_chat_id), content_hash = VALUES(content_hash), status = 'pending', attempt_count = attempt_count + 1, last_error = NULL")
            .bind(&route.id).bind(message.id.to_string()).bind(message.channel_id.to_string()).bind(&route.telegram_chat_id).bind(&hash).execute(&database.link).await?;
        if !old_references.is_empty()
            && let Err(error) = self
                .client
                .delete(&route.telegram_chat_id, &old_references)
                .await
        {
            sqlx::query("UPDATE telegram_crossposts SET status = 'failed', telegram_messages = ?, last_error = ? WHERE route_id = ? AND discord_message_id = ?")
                .bind(serde_json::to_string(&old_references)?).bind(error.to_string()).bind(&route.id).bind(message.id.to_string()).execute(&database.link).await?;
            return Err(error);
        }
        match self.client.send(route, &payload).await {
            Ok(references) => {
                sqlx::query("UPDATE telegram_crossposts SET status = 'sent', telegram_messages = ?, last_error = NULL WHERE route_id = ? AND discord_message_id = ?")
                    .bind(serde_json::to_string(&references)?).bind(&route.id).bind(message.id.to_string()).execute(&database.link).await?;
                self.advance_checkpoint(route, message.id).await?;
                tracing::info!(route = route.id, message_id = %message.id, telegram_messages = references.len(), "Telegram crosspost delivered");
                Ok(())
            }
            Err(failure) => {
                sqlx::query("UPDATE telegram_crossposts SET status = 'failed', telegram_messages = ?, last_error = ? WHERE route_id = ? AND discord_message_id = ?")
                    .bind(serde_json::to_string(&failure.references)?).bind(failure.error.to_string()).bind(&route.id).bind(message.id.to_string()).execute(&database.link).await?;
                Err(failure.error)
            }
        }
    }

    async fn crosspost(
        &self,
        route: &TelegramRoute,
        message_id: serenity::MessageId,
    ) -> Result<Option<CrosspostRow>> {
        let database = self
            .databases
            .as_ref()
            .context("Telegram storage is unavailable")?;
        sqlx::query_as::<_, CrosspostRow>("SELECT content_hash, status, telegram_messages FROM telegram_crossposts WHERE route_id = ? AND discord_message_id = ? LIMIT 1")
            .bind(&route.id).bind(message_id.to_string()).fetch_optional(&database.link).await.context("failed to read Telegram crosspost")
    }

    async fn set_checkpoint(
        &self,
        route: &TelegramRoute,
        message_id: serenity::MessageId,
    ) -> Result<()> {
        let database = self
            .databases
            .as_ref()
            .context("Telegram storage is unavailable")?;
        sqlx::query("INSERT INTO telegram_crosspost_routes (route_id, last_discord_message_id) VALUES (?, ?) ON DUPLICATE KEY UPDATE last_discord_message_id = VALUES(last_discord_message_id)")
            .bind(&route.id).bind(message_id.to_string()).execute(&database.link).await?;
        Ok(())
    }

    async fn advance_checkpoint(
        &self,
        route: &TelegramRoute,
        message_id: serenity::MessageId,
    ) -> Result<()> {
        let current = self.route_checkpoint(route).await?;
        if current.is_none_or(|current| current < message_id.get()) {
            self.set_checkpoint(route, message_id).await?;
        }
        Ok(())
    }

    async fn route_checkpoint(&self, route: &TelegramRoute) -> Result<Option<u64>> {
        let database = self
            .databases
            .as_ref()
            .context("Telegram storage is unavailable")?;
        Ok(sqlx::query_scalar::<_, Option<String>>(
            "SELECT last_discord_message_id FROM telegram_crosspost_routes WHERE route_id = ? LIMIT 1",
        )
        .bind(&route.id)
        .fetch_optional(&database.link)
        .await?
        .flatten()
        .and_then(|value| value.parse().ok()))
    }

    async fn initialize_route(&self, ctx: &serenity::Context, route: &TelegramRoute) -> Result<()> {
        let database = self
            .databases
            .as_ref()
            .context("Telegram storage is unavailable")?;
        let checkpoint = sqlx::query_scalar::<_, Option<String>>("SELECT last_discord_message_id FROM telegram_crosspost_routes WHERE route_id = ? LIMIT 1")
            .bind(&route.id).fetch_optional(&database.link).await?.flatten();
        if let Some(checkpoint) = checkpoint {
            if let Ok(checkpoint) = checkpoint.parse::<u64>() {
                self.backfill_route(ctx, route, serenity::MessageId::new(checkpoint))
                    .await?;
            }
            return Ok(());
        }
        let latest = route
            .discord_channel_id
            .messages(ctx, serenity::GetMessages::new().limit(1))
            .await?
            .into_iter()
            .next();
        if let Some(message) = latest {
            if self.config.backfill_on_first_run {
                self.deliver(route, &message, false).await?;
            } else {
                self.set_checkpoint(route, message.id).await?;
            }
        } else {
            sqlx::query("INSERT IGNORE INTO telegram_crosspost_routes (route_id, last_discord_message_id) VALUES (?, NULL)").bind(&route.id).execute(&database.link).await?;
        }
        Ok(())
    }

    async fn backfill_route(
        &self,
        ctx: &serenity::Context,
        route: &TelegramRoute,
        mut after: serenity::MessageId,
    ) -> Result<()> {
        loop {
            if self.is_shutting_down() {
                return Ok(());
            }
            let mut messages = route
                .discord_channel_id
                .messages(ctx, serenity::GetMessages::new().after(after).limit(100))
                .await?;
            if messages.is_empty() {
                return Ok(());
            }
            messages.sort_by_key(|message| message.id);
            let page_size = messages.len();
            for message in messages {
                if self.is_shutting_down() {
                    return Ok(());
                }
                after = message.id;
                if let Err(error) = self.deliver(route, &message, false).await {
                    tracing::error!(%error, route = route.id, message_id = %message.id, "failed to backfill Telegram crosspost");
                }
            }
            if page_size < 100 {
                return Ok(());
            }
        }
    }

    async fn recover(&self, ctx: &serenity::Context) -> Result<()> {
        let database = self
            .databases
            .as_ref()
            .context("Telegram storage is unavailable")?;
        let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as("SELECT route_id, discord_message_id, discord_channel_id, telegram_chat_id, telegram_messages FROM telegram_crossposts WHERE status IN ('pending', 'failed') ORDER BY updated_at ASC LIMIT 200").fetch_all(&database.link).await?;
        for (route_id, message_id, channel_id, telegram_chat_id, telegram_messages) in rows {
            if self.is_shutting_down() {
                return Ok(());
            }
            let Some(route) = self.config.routes.iter().find(|route| route.id == route_id) else {
                continue;
            };
            let Ok(message_id) = message_id.parse::<u64>() else {
                continue;
            };
            let checkpoint = self.route_checkpoint(route).await?;
            if checkpoint.is_some_and(|checkpoint| message_id <= checkpoint) {
                sqlx::query("UPDATE telegram_crossposts SET status = 'ignored', last_error = NULL WHERE route_id = ? AND discord_message_id = ?")
                    .bind(&route.id)
                    .bind(message_id.to_string())
                    .execute(&database.link)
                    .await?;
                tracing::info!(
                    route = route.id,
                    message_id,
                    ?checkpoint,
                    "retired historical Telegram recovery row at or before the route checkpoint"
                );
                continue;
            }
            let Ok(channel_id) = channel_id.parse::<u64>() else {
                continue;
            };
            match serenity::ChannelId::new(channel_id)
                .message(ctx, serenity::MessageId::new(message_id))
                .await
            {
                Ok(message) => {
                    if let Err(error) = self.deliver(route, &message, false).await {
                        tracing::error!(%error, route = route.id, message_id, "failed to recover Telegram crosspost");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, route = route.id, message_id, "could not fetch Discord message for Telegram recovery");
                    if let Err(delete_error) = self
                        .client
                        .delete(
                            &telegram_chat_id,
                            &parse_references(telegram_messages.as_deref()),
                        )
                        .await
                    {
                        tracing::error!(%delete_error, route = route.id, message_id, "failed to delete orphaned Telegram crosspost");
                        continue;
                    }
                    if let Err(database_error) = sqlx::query("UPDATE telegram_crossposts SET status = 'deleted', last_error = NULL WHERE route_id = ? AND discord_message_id = ?")
                        .bind(&route.id).bind(message_id.to_string()).execute(&database.link).await
                    {
                        tracing::error!(%database_error, route = route.id, message_id, "failed to mark Telegram crosspost deleted");
                    }
                }
            }
        }
        Ok(())
    }
}

fn is_after_checkpoint(message_id: serenity::MessageId, checkpoint: Option<u64>) -> bool {
    checkpoint.is_some_and(|checkpoint| message_id.get() > checkpoint)
}

fn reap_completed_tasks(tasks: &mut JoinSet<()>) {
    while let Some(result) = tasks.try_join_next() {
        if let Err(error) = result {
            tracing::error!(%error, "Telegram delivery task failed");
        }
    }
}

impl TelegramClient {
    async fn send(
        &self,
        route: &TelegramRoute,
        payload: &Payload,
    ) -> std::result::Result<Vec<TelegramReference>, DeliveryFailure> {
        let mut references = Vec::new();
        for text in &payload.text {
            let message: TelegramMessage = match self.request("sendMessage", &route.telegram_chat_id, serde_json::json!({ "chat_id": route.telegram_chat_id, "message_thread_id": route.telegram_thread_id, "text": text, "parse_mode": "HTML", "link_preview_options": { "is_disabled": false } })).await {
                Ok(message) => message,
                Err(error) => return Err(DeliveryFailure { error, references }),
            };
            references.push(TelegramReference {
                message_id: message.message_id,
                kind: TelegramMessageKind::Text,
            });
        }
        for attachment in &payload.attachments {
            let (method, field, kind) = attachment_method(attachment);
            let mut value = serde_json::json!({ "chat_id": route.telegram_chat_id, "message_thread_id": route.telegram_thread_id });
            value[field] = Value::String(attachment.url.clone());
            let message: TelegramMessage =
                match self.request(method, &route.telegram_chat_id, value).await {
                    Ok(message) => message,
                    Err(error) => return Err(DeliveryFailure { error, references }),
                };
            references.push(TelegramReference {
                message_id: message.message_id,
                kind,
            });
        }
        Ok(references)
    }

    async fn delete(&self, chat_id: &str, references: &[TelegramReference]) -> Result<()> {
        for reference in references {
            let result: Result<bool> = self
                .request(
                    "deleteMessage",
                    chat_id,
                    serde_json::json!({ "chat_id": chat_id, "message_id": reference.message_id }),
                )
                .await;
            if let Err(error) = result
                && !error
                    .to_string()
                    .to_ascii_lowercase()
                    .contains("message to delete not found")
            {
                return Err(error);
            }
        }
        Ok(())
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        chat_id: &str,
        payload: Value,
    ) -> Result<T> {
        let mut last_error = None;
        for attempt in 1..=self.retry_attempts {
            self.throttle(chat_id).await;
            let response = self
                .http
                .post(format!("{}/{method}", self.base_url))
                .json(&payload)
                .send()
                .await;
            match response {
                Ok(response) => {
                    let status = response.status();
                    match response.json::<TelegramResponse<T>>().await {
                        Ok(body) if status.is_success() && body.ok => {
                            return body.result.context("Telegram returned no result");
                        }
                        Ok(body) => {
                            let permanent = body
                                .error_code
                                .is_some_and(|code| (400..500).contains(&code) && code != 429);
                            let delay = body
                                .parameters
                                .and_then(|value| value.retry_after)
                                .map(|seconds| Duration::from_millis(seconds * 1_000 + 250));
                            last_error = Some(anyhow::anyhow!(
                                "{}",
                                body.description
                                    .unwrap_or_else(|| format!("Telegram {method} failed"))
                            ));
                            if permanent {
                                break;
                            }
                            if attempt < self.retry_attempts {
                                tokio::time::sleep(delay.unwrap_or_else(|| retry_delay(attempt)))
                                    .await;
                            }
                        }
                        Err(error) => {
                            last_error = Some(error.into());
                            if attempt < self.retry_attempts {
                                tokio::time::sleep(retry_delay(attempt)).await;
                            }
                        }
                    }
                }
                Err(error) => {
                    last_error = Some(error.into());
                    if attempt < self.retry_attempts {
                        tokio::time::sleep(retry_delay(attempt)).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Telegram {method} failed")))
    }

    async fn throttle(&self, chat_id: &str) {
        let mut requests = self.last_request.lock().await;
        if let Some(last) = requests.get(chat_id) {
            let elapsed = last.elapsed();
            if elapsed < Duration::from_millis(1_050) {
                tokio::time::sleep(Duration::from_millis(1_050).saturating_sub(elapsed)).await;
            }
        }
        requests.insert(chat_id.to_owned(), std::time::Instant::now());
    }
}

fn retry_delay(attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(31);
    Duration::from_millis((1_000_u64.saturating_mul(2_u64.saturating_pow(exponent))).min(30_000))
}

fn build_payload(route: &TelegramRoute, message: &serenity::Message) -> Payload {
    let mut parts = Vec::new();
    if route.include_author {
        parts.push(format!(
            "**Posted by {}**",
            message
                .author
                .global_name
                .as_deref()
                .unwrap_or(&message.author.name),
        ));
    }
    let content = message.content.trim().to_owned();
    if !content.is_empty() {
        parts.push(content);
    }
    let mut attachments = message
        .attachments
        .iter()
        .map(|attachment| Attachment {
            filename: attachment.filename.clone(),
            url: attachment.url.clone(),
            content_type: attachment.content_type.clone(),
        })
        .collect::<Vec<_>>();
    let mut attachment_urls = attachments
        .iter()
        .map(|attachment| attachment.url.clone())
        .collect::<std::collections::HashSet<_>>();
    for (index, embed) in message.embeds.iter().enumerate() {
        let mut embed_parts = Vec::new();
        if let Some(title) = &embed.title {
            embed_parts.push(format!("**{title}**"));
        }
        if let Some(description) = &embed.description {
            embed_parts.push(description.clone());
        }
        for field in &embed.fields {
            embed_parts.push(format!("**{}**\n{}", field.name, field.value));
        }
        if let Some(url) = &embed.url
            && !embed_parts.iter().any(|part| part.contains(url))
        {
            embed_parts.push(url.clone());
        }
        for (kind, url) in [
            (
                "image",
                embed.image.as_ref().map(|image| image.url.as_str()),
            ),
            (
                "thumbnail",
                embed.thumbnail.as_ref().map(|image| image.url.as_str()),
            ),
        ] {
            let Some(url) = url else { continue };
            if attachment_urls.insert(url.to_owned()) {
                let filename = reqwest::Url::parse(url)
                    .ok()
                    .and_then(|url| {
                        url.path_segments()?
                            .next_back()
                            .filter(|name| !name.is_empty())
                            .map(ToOwned::to_owned)
                    })
                    .unwrap_or_else(|| format!("embed-{kind}-{}", index + 1));
                attachments.push(Attachment {
                    filename,
                    url: url.to_owned(),
                    content_type: None,
                });
            }
        }
        let embed = embed_parts.join("\n\n");
        if !embed.is_empty() {
            parts.push(embed);
        }
    }
    let text = rewrite_telegram_links(&parts.join("\n\n"), route);
    Payload {
        text: split_text(&text, TEXT_LIMIT)
            .into_iter()
            .map(|chunk| format_telegram_html(&chunk))
            .filter(|chunk| !chunk.is_empty())
            .collect(),
        attachments,
    }
}

fn rewrite_telegram_links(value: &str, route: &TelegramRoute) -> String {
    static URL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let url =
        URL.get_or_init(|| Regex::new(r#"https?://[^\s<>\]\)\"']+"#).expect("valid URL regex"));
    let mut internal_link_index = 0_usize;
    url.replace_all(value, |captures: &regex::Captures<'_>| {
        let matched = captures.get(0).map_or("", |value| value.as_str());
        let trimmed = matched.trim_end_matches(['.', ',', '!', '?', ';', ':']);
        let trailing = &matched[trimmed.len()..];
        rewrite_telegram_url(trimmed, route, internal_link_index).map_or_else(
            || matched.to_owned(),
            |rewritten| {
                internal_link_index += 1;
                format!("{rewritten}{trailing}")
            },
        )
    })
    .into_owned()
}

fn rewrite_telegram_url(
    value: &str,
    route: &TelegramRoute,
    internal_link_index: usize,
) -> Option<String> {
    let mut url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.host_str(), Some("6b6t.org" | "www.6b6t.org")) {
        return None;
    }
    url.set_scheme("https").ok()?;
    url.set_host(Some("www.6b6t.org")).ok()?;
    url.set_port(None).ok()?;

    let existing = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let campaign = existing
        .iter()
        .find(|(name, _)| name == "utm_campaign")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| telegram_campaign(&url).to_owned());
    let retained = existing
        .into_iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "utm_source" | "utm_medium" | "utm_campaign" | "utm_content" | "lang"
            )
        })
        .collect::<Vec<_>>();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (name, value) in retained {
            query.append_pair(&name, &value);
        }
        query
            .append_pair("utm_source", "telegram")
            .append_pair("utm_medium", "telegram_group")
            .append_pair("utm_campaign", &campaign)
            .append_pair(
                "utm_content",
                &format!(
                    "org6b6t_{}_{}",
                    telegram_utm_topic(route),
                    if internal_link_index == 0 {
                        "main"
                    } else {
                        "footer"
                    }
                ),
            )
            .append_pair("lang", route.utm_language.as_deref().unwrap_or("en"));
    }
    Some(url.into())
}

fn telegram_campaign(url: &reqwest::Url) -> &'static str {
    let mut segments = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty());
    let first = segments.next().unwrap_or_default();
    let section = if first.len() == 2 {
        segments.next().unwrap_or(first)
    } else {
        first
    };
    match section.to_ascii_lowercase().as_str() {
        "shop" => "evergreen_shop",
        "vote" => "evergreen_vote",
        _ => "evergreen_website",
    }
}

fn telegram_utm_topic(route: &TelegramRoute) -> String {
    if let Some(topic) = &route.utm_topic {
        return topic.clone();
    }
    match route.id.as_str() {
        "announcement" | "announcements" | "news" | "updates" => "news".to_owned(),
        _ => route
            .id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_owned(),
    }
}

fn format_telegram_html(value: &str) -> String {
    static PATTERNS: std::sync::OnceLock<Vec<(Regex, &'static str)>> = std::sync::OnceLock::new();
    static TIMESTAMP: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        vec![
            (
                Regex::new(r"<a?:[A-Za-z0-9_]+:\d+>").expect("valid regex"),
                "",
            ),
            (
                Regex::new(r"<@!?\d+>|<@&\d+>|<#\d+>").expect("valid regex"),
                "",
            ),
            (Regex::new(r"</([^:>]+):\d+>").expect("valid regex"), "/$1"),
            (
                Regex::new(r"(?m)^#{1,6}\s+(.+)$").expect("valid regex"),
                "**$1**",
            ),
            (Regex::new(r"(?m)^>\s?").expect("valid regex"), ""),
            (
                Regex::new(r"(?im)(^|[\s(\[{:])@(everyone|here)\b").expect("valid regex"),
                "$1",
            ),
            (
                Regex::new(r"(?m)(^|[\s(\[{:])@[A-Za-z0-9_]{5,32}\b").expect("valid regex"),
                "$1",
            ),
            (Regex::new(r"[ \t]{2,}").expect("valid regex"), " "),
            (Regex::new(r"[ \t]+\n").expect("valid regex"), "\n"),
            (Regex::new(r"\n{3,}").expect("valid regex"), "\n\n"),
        ]
    });
    let timestamp = TIMESTAMP
        .get_or_init(|| Regex::new(r"<t:(-?\d+)(?::[tTdDfFR])?>").expect("valid timestamp regex"));
    let value = timestamp.replace_all(value, |captures: &regex::Captures<'_>| {
        captures
            .get(1)
            .and_then(|value| value.as_str().parse::<i64>().ok())
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .map_or_else(String::new, |date| {
                date.format("%Y-%m-%d %H:%M UTC").to_string()
            })
    });
    let normalized = patterns
        .iter()
        .fold(value.into_owned(), |text, (regex, replacement)| {
            regex.replace_all(&text, *replacement).into_owned()
        });
    render_discord_markdown_as_html(normalized.trim())
}

fn render_discord_markdown_as_html(value: &str) -> String {
    static CODE_BLOCK: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static INLINE_CODE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static MARKDOWN_LINK: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static FORMATTING: std::sync::OnceLock<Vec<(Regex, &'static str)>> = std::sync::OnceLock::new();

    let mut placeholders = Vec::new();
    let text = CODE_BLOCK
        .get_or_init(|| Regex::new(r"```(?:[A-Za-z0-9_-]+)?\n?([\s\S]*?)```").expect("valid regex"))
        .replace_all(value, |captures: &regex::Captures<'_>| {
            html_placeholder(
                &mut placeholders,
                format!(
                    "<pre>{}</pre>",
                    html_escape::encode_text(captures.get(1).map_or("", |value| value.as_str()))
                ),
            )
        })
        .into_owned();
    let text = INLINE_CODE
        .get_or_init(|| Regex::new(r"`([^`\n]+)`").expect("valid regex"))
        .replace_all(&text, |captures: &regex::Captures<'_>| {
            html_placeholder(
                &mut placeholders,
                format!(
                    "<code>{}</code>",
                    html_escape::encode_text(captures.get(1).map_or("", |value| value.as_str()))
                ),
            )
        })
        .into_owned();
    let text = MARKDOWN_LINK
        .get_or_init(|| {
            Regex::new(r"\[([^\]\n]+)]\(<?(https?://[^)>\s]+)>?\)").expect("valid regex")
        })
        .replace_all(&text, |captures: &regex::Captures<'_>| {
            let label = captures.get(1).map_or("", |value| value.as_str());
            let url = captures.get(2).map_or("", |value| value.as_str());
            html_placeholder(
                &mut placeholders,
                format!(
                    "<a href=\"{}\">{}</a>",
                    html_escape::encode_double_quoted_attribute(url),
                    html_escape::encode_text(label)
                ),
            )
        })
        .into_owned();

    let mut html = html_escape::encode_text(&text).into_owned();
    for (regex, replacement) in FORMATTING.get_or_init(|| {
        vec![
            (
                Regex::new(r"\*\*([^*\n]+)\*\*").expect("valid regex"),
                "<b>$1</b>",
            ),
            (
                Regex::new(r"__([^_\n]+)__").expect("valid regex"),
                "<u>$1</u>",
            ),
            (
                Regex::new(r"~~([^~\n]+)~~").expect("valid regex"),
                "<s>$1</s>",
            ),
            (
                Regex::new(r"\|\|([^|\n]+)\|\|").expect("valid regex"),
                "<tg-spoiler>$1</tg-spoiler>",
            ),
            (
                Regex::new(r"(?m)(^|\s)\*([^*\n]+)\*(\s|$)").expect("valid regex"),
                "$1<i>$2</i>$3",
            ),
            (
                Regex::new(r"(?m)(^|\s)_([^_\n]+)_(\s|$)").expect("valid regex"),
                "$1<i>$2</i>$3",
            ),
        ]
    }) {
        html = regex.replace_all(&html, *replacement).into_owned();
    }
    for (index, replacement) in placeholders.iter().enumerate() {
        html = html.replace(&format!("\u{e000}{index}\u{e001}"), replacement);
    }
    html.trim().to_owned()
}

fn html_placeholder(placeholders: &mut Vec<String>, html: String) -> String {
    let index = placeholders.len();
    placeholders.push(html);
    format!("\u{e000}{index}\u{e001}")
}

fn split_text(value: &str, limit: usize) -> Vec<String> {
    if value.trim().is_empty() {
        return Vec::new();
    }
    let mut chars = value.trim().chars().collect::<Vec<_>>();
    let mut chunks = Vec::new();
    while !chars.is_empty() {
        let end = chars.len().min(limit);
        let mut split = end;
        if chars.len() > limit
            && let Some(position) = chars[..end]
                .iter()
                .rposition(|character| character.is_whitespace())
            && position >= limit * 2 / 5
        {
            split = position;
        }
        chunks.push(chars.drain(..split).collect::<String>().trim().to_owned());
        while chars
            .first()
            .is_some_and(|character| character.is_whitespace())
        {
            chars.remove(0);
        }
    }
    chunks
}

fn payload_hash(payload: &Payload) -> String {
    let mut hasher = Sha256::new();
    for text in &payload.text {
        hasher.update(text.as_bytes());
        hasher.update([0]);
    }
    for attachment in &payload.attachments {
        hasher.update(stable_url(&attachment.url).as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

fn stable_url(value: &str) -> String {
    reqwest::Url::parse(value).map_or_else(
        |_| value.split('?').next().unwrap_or(value).to_owned(),
        |url| {
            format!(
                "{}://{}{}",
                url.scheme(),
                url.host_str().unwrap_or_default(),
                url.path()
            )
        },
    )
}

fn attachment_method(attachment: &Attachment) -> (&'static str, &'static str, TelegramMessageKind) {
    let content_type = attachment
        .content_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let filename = attachment.filename.to_ascii_lowercase();
    if content_type.starts_with("image/")
        || [".jpg", ".jpeg", ".png"]
            .iter()
            .any(|extension| filename.ends_with(extension))
    {
        ("sendPhoto", "photo", TelegramMessageKind::Photo)
    } else if content_type.starts_with("video/")
        || [".mp4", ".mov"]
            .iter()
            .any(|extension| filename.ends_with(extension))
    {
        ("sendVideo", "video", TelegramMessageKind::Video)
    } else {
        ("sendDocument", "document", TelegramMessageKind::Document)
    }
}

fn parse_references(value: Option<&str>) -> Vec<TelegramReference> {
    value
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use poise::serenity_prelude as serenity;

    use super::{format_telegram_html, is_after_checkpoint, rewrite_telegram_links, split_text};
    use crate::config::TelegramRoute;

    fn route() -> TelegramRoute {
        TelegramRoute {
            id: "announcements".to_owned(),
            discord_channel_id: serenity::ChannelId::new(1),
            telegram_chat_id: "@org6b6t".to_owned(),
            telegram_thread_id: Some(1),
            include_author: false,
            utm_topic: None,
            utm_language: None,
        }
    }

    #[test]
    fn only_updates_after_the_checkpoint_can_create_crossposts() {
        assert!(is_after_checkpoint(
            serenity::MessageId::new(101),
            Some(100)
        ));
        assert!(!is_after_checkpoint(
            serenity::MessageId::new(100),
            Some(100)
        ));
        assert!(!is_after_checkpoint(
            serenity::MessageId::new(99),
            Some(100)
        ));
        assert!(!is_after_checkpoint(serenity::MessageId::new(101), None));
    }

    #[test]
    fn markdown_becomes_safe_telegram_html() {
        assert_eq!(
            format_telegram_html("**Update** <@123> [Shop](https://www.6b6t.org/shop?a=1&b=2)"),
            "<b>Update</b> <a href=\"https://www.6b6t.org/shop?a=1&amp;b=2\">Shop</a>"
        );
    }
    #[test]
    fn markdown_preserves_emails_and_strips_safe_mentions_and_italics() {
        assert_eq!(
            format_telegram_html(
                "mail@example.com @everyone *important update* @username  \nnext line"
            ),
            "mail@example.com <i>important update</i>\nnext line"
        );
    }
    #[test]
    fn internal_links_receive_telegram_utms() {
        let text = rewrite_telegram_links(
            "[Shop](https://6b6t.org/shop?utm_source=discord&utm_medium=discord_channel&utm_campaign=event_christmas_2026&utm_content=updates_main&lang=en) https://www.6b6t.org/vote",
            &route(),
        );
        assert_eq!(
            text,
            "[Shop](https://www.6b6t.org/shop?utm_source=telegram&utm_medium=telegram_group&utm_campaign=event_christmas_2026&utm_content=org6b6t_news_main&lang=en) https://www.6b6t.org/vote?utm_source=telegram&utm_medium=telegram_group&utm_campaign=evergreen_vote&utm_content=org6b6t_news_footer&lang=en"
        );
    }
    #[test]
    fn external_and_blog_links_are_not_rewritten() {
        let text = "https://blog.6b6t.org/post https://youtube.com/watch?v=1";
        assert_eq!(rewrite_telegram_links(text, &route()), text);
    }
    #[test]
    fn utm_rewrite_preserves_query_data_fragments_and_route_language() {
        let mut route = route();
        route.utm_topic = Some("general".to_owned());
        route.utm_language = Some("es".to_owned());
        assert_eq!(
            rewrite_telegram_links(
                "https://www.6b6t.org/en/stats/player?view=compact&utm_source=discord#history",
                &route,
            ),
            "https://www.6b6t.org/en/stats/player?view=compact&utm_source=telegram&utm_medium=telegram_group&utm_campaign=evergreen_website&utm_content=org6b6t_general_main&lang=es#history"
        );
    }
    #[test]
    fn text_splits_without_losing_unicode() {
        let chunks = split_text("one two three", 7);
        assert_eq!(chunks, ["one", "two", "three"]);
        assert_eq!(split_text("🎉🎉", 1), ["🎉", "🎉"]);
    }
}
