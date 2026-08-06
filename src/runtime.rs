use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::{
    commands, config, events,
    state::{AppState, Error},
};

pub async fn start(state: AppState) -> Result<()> {
    let token = state.environment.discord_token.clone();
    let setup_state = state.clone();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            event_handler: |ctx, event, _framework, data| Box::pin(events::handle(ctx, event, data)),
            on_error: |error| Box::pin(handle_framework_error(error)),
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            let state = setup_state.clone();
            Box::pin(async move {
                poise::builtins::register_in_guild(ctx, &framework.options().commands, config::GUILD_ID).await?;
                tracing::info!(user = %ready.user.tag(), commands = framework.options().commands.len(), "registered guild application commands");
                Ok(state)
            })
        })
        .build();

    let intents = serenity::GatewayIntents::GUILDS
        | serenity::GatewayIntents::GUILD_MEMBERS
        | serenity::GatewayIntents::GUILD_MESSAGES
        | serenity::GatewayIntents::GUILD_MESSAGE_REACTIONS
        | serenity::GatewayIntents::MESSAGE_CONTENT;
    let mut client = serenity::ClientBuilder::new(token, intents)
        .application_id(serenity::ApplicationId::new(config::APPLICATION_ID))
        .framework(framework)
        .await
        .context("failed to create Discord client")?;

    let result = tokio::select! {
        result = client.start() => result.context("Discord client stopped with an error"),
        signal = shutdown_signal() => {
            match signal {
                Ok(signal) => {
                    tracing::info!(signal, "received shutdown signal");
                    client.shard_manager.shutdown_all().await;
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
    };
    state.shutdown().await;
    result
}

#[cfg(unix)]
async fn shutdown_signal() -> Result<&'static str> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("failed to listen for SIGTERM")?;
    let mut user_defined =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2())
            .context("failed to listen for SIGUSR2")?;
    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.context("failed to listen for Ctrl+C")?;
            Ok("SIGINT")
        }
        _ = terminate.recv() => Ok("SIGTERM"),
        _ = user_defined.recv() => Ok("SIGUSR2"),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> Result<&'static str> {
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for Ctrl+C")?;
    Ok("Ctrl+C")
}

async fn handle_framework_error(error: poise::FrameworkError<'_, AppState, Error>) {
    tracing::error!(%error, "Discord framework error");
    if let Err(reporting_error) = poise::builtins::on_error(error).await {
        tracing::error!(%reporting_error, "failed to report Discord framework error");
    }
}
