use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context as _, Result};
use tokio::sync::{Mutex, RwLock};

use crate::{
    anarchy::AnarchyService, community_event::CommunityEventService, config::Environment,
    database::Databases, event_submissions::EventSubmissionService, media::MediaState,
    moderation::PendingApprovals, server::ServerService, telegram::TelegramService,
    youtube::YoutubeService,
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
    pub anarchy: Option<AnarchyService>,
    pub community_event: Option<CommunityEventService>,
    pub event_submissions: Option<EventSubmissionService>,
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
            match Databases::connect(config).await {
                Ok(databases) => Some(databases),
                Err(error) => {
                    tracing::error!(%error, "MySQL initialization failed; database features are disabled");
                    None
                }
            }
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
        let anarchy = load_anarchy(&environment);
        let community_event = load_community_event(&environment);
        let event_submissions = match (environment.event_channels, databases.clone()) {
            (Some(channels), Some(databases)) => Some(EventSubmissionService::new(
                channels,
                databases,
                environment.events_test_user_id,
            )),
            (Some(_), None) => {
                tracing::error!("community events require MySQL; event submissions are disabled");
                None
            }
            (None, _) => None,
        };

        Ok(Self {
            server: ServerService::new(http.clone(), Arc::clone(&environment), databases.clone()),
            youtube: YoutubeService::new(environment.youtube_api_key.clone())?,
            environment,
            http,
            databases,
            media: Arc::new(MediaState::load().await?),
            pending: Arc::new(PendingApprovals::default()),
            telegram,
            anarchy,
            community_event,
            event_submissions,
            role_sync_cache: Arc::new(RwLock::new(HashMap::new())),
            ready_started: Arc::new(Mutex::new(false)),
        })
    }

    pub async fn shutdown(&self) {
        if let Some(telegram) = &self.telegram {
            telegram.shutdown().await;
        }
        if let Some(databases) = &self.databases {
            databases.link.close().await;
            databases.stats.close().await;
        }
    }
}

fn load_community_event(environment: &Arc<Environment>) -> Option<CommunityEventService> {
    if !environment.community_event_announcements_enabled {
        return None;
    }
    let channel_id = environment.community_event_announcement_channel_id?;
    let Some(redis) = environment.redis.as_ref() else {
        tracing::error!(
            "community-event Discord announcements require Redis; announcements are disabled"
        );
        return None;
    };
    match CommunityEventService::new(
        redis,
        channel_id,
        environment.community_event_announcement_channel_id_es,
        environment.community_event_announcement_channel_id_de,
        environment.community_event_announcement_channel_id_tr,
        environment.community_event_announcement_channel_id_dupe,
    ) {
        Ok(service) => Some(service),
        Err(error) => {
            tracing::error!(%error, "failed to initialize community-event Discord announcements; disabled");
            None
        }
    }
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, AppState, Error>;

fn load_anarchy(environment: &Arc<Environment>) -> Option<AnarchyService> {
    let (Some(channel_id), Some(redis)) = (
        environment.anarchy_analytics_channel_id,
        environment.redis.as_ref(),
    ) else {
        tracing::warn!(
            "anarchy analytics require ANARCHY_ANALYTICS_CHANNEL_ID and REDIS_HOST; analytics are disabled"
        );
        return None;
    };
    match AnarchyService::new(redis, channel_id) {
        Ok(service) => Some(service),
        Err(error) => {
            tracing::error!(%error, "failed to initialize anarchy mod analytics; disabled");
            None
        }
    }
}
