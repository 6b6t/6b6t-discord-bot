use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use tokio::sync::{Mutex, RwLock};

use crate::{
    config::Environment, database::Databases, media::MediaState, moderation::PendingApprovals,
    server::ServerService, telegram::TelegramService, youtube::YoutubeService,
};

#[derive(Clone)]
pub struct AppState {
    pub environment: Arc<Environment>,
    pub http: reqwest::Client,
    pub databases: Option<Databases>,
    pub media: Arc<MediaState>,
    pub pending: Arc<PendingApprovals>,
    pub server: ServerService,
    pub telegram: Option<TelegramService>,
    pub youtube: YoutubeService,
    pub role_sync_cache: Arc<RwLock<HashMap<String, CachedUserInfo>>>,
    pub ready_started: Arc<Mutex<bool>>,
}

#[derive(Clone)]
pub struct CachedUserInfo {
    pub value: crate::server::UserInfo,
    pub expires_at: std::time::Instant,
}

impl AppState {
    pub async fn load() -> Result<Self> {
        let environment = Arc::new(Environment::load()?);
        let http = reqwest::Client::builder()
            .user_agent(concat!("6b6t-discord-bot/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        let databases = if let Some(config) = &environment.database {
            Some(Databases::connect(config).await?)
        } else {
            tracing::warn!(
                "MySQL is not configured; linking, role sync, and Telegram storage are disabled"
            );
            None
        };
        let telegram = environment
            .telegram
            .as_ref()
            .map(|config| TelegramService::new(http.clone(), config.clone(), databases.clone()));

        Ok(Self {
            server: ServerService::new(http.clone(), Arc::clone(&environment), databases.clone()),
            youtube: YoutubeService::new(environment.youtube_api_key.clone())?,
            environment,
            http,
            databases,
            media: Arc::new(MediaState::load().await?),
            pending: Arc::new(PendingApprovals::default()),
            telegram,
            role_sync_cache: Arc::new(RwLock::new(HashMap::new())),
            ready_started: Arc::new(Mutex::new(false)),
        })
    }
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, AppState, Error>;
