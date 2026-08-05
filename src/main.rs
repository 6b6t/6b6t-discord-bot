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
    install_crypto_provider()?;
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

fn install_crypto_provider() -> Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return Ok(());
    }
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("failed to install the Rustls Ring crypto provider"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_clients_initialize_after_crypto_provider_installation() {
        install_crypto_provider().expect("the crypto provider should install");
        reqwest::Client::builder()
            .build()
            .expect("the shared HTTP client should initialize");
        crate::youtube::YoutubeService::new(None).expect("the YouTube client should initialize");
    }
}
