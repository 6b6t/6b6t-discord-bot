mod command_moderation;
mod commands;
mod config;
mod database;
mod events;
mod media;
mod moderation;
mod runtime;
mod server;
mod state;
mod telegram;
mod youtube;

use anyhow::{Context as _, Result, anyhow};
use state::AppState;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("sixbsixt_discord_bot=info,warn")),
        )
        .try_init()
        .map_err(|error| anyhow!("failed to initialize tracing: {error}"))?;

    let state = AppState::load()
        .await
        .context("failed to initialize bot services")?;
    runtime::start(state).await
}
