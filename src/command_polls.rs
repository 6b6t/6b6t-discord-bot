//! `/poll create|close|results`: staff commands for eligibility-gated polls.

use std::fmt::Write as _;

use anyhow::Context as _;
use poise::{CreateReply, serenity_prelude as serenity};

use crate::{
    config, moderation,
    polls::service::{CloseOutcome, CreateOutcome, CreateRequest, PollService, parse_duration},
    state::{Context, Error},
};

/// Eligibility-gated button polls (staff only).
#[poise::command(slash_command, guild_only, subcommands("create", "close", "results"))]
#[allow(clippy::unused_async)]
pub async fn poll(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Start a poll that only eligible players can vote in.
#[poise::command(slash_command, guild_only)]
#[allow(clippy::too_many_arguments)]
async fn create(
    ctx: Context<'_>,
    #[description = "The question (up to 200 characters)"] question: String,
    #[description = "First option"] option_1: String,
    #[description = "Second option"] option_2: String,
    #[description = "Who can vote, e.g. `veteran AND active` or `crystal_pvper`"] requires: String,
    #[description = "How long the poll runs, e.g. 3d, 12h, 90m or 1d12h"] duration: String,
    #[description = "Channel to post in (default: this channel)"] channel: Option<
        serenity::GuildChannel,
    >,
    #[description = "Third option"] option_3: Option<String>,
    #[description = "Fourth option"] option_4: Option<String>,
    #[description = "Fifth option"] option_5: Option<String>,
) -> Result<(), Error> {
    let Some(service) = require_staff_and_service(ctx).await? else {
        return Ok(());
    };
    let Some(duration_seconds) = parse_duration(&duration) else {
        reply(
            ctx,
            "I could not read that duration. Use for example `3d`, `12h`, `90m` or `1d12h`.",
        )
        .await?;
        return Ok(());
    };
    let channel_id = channel
        .as_ref()
        .map_or_else(|| ctx.channel_id(), |channel| channel.id);
    if let Some(channel) = &channel
        && !matches!(
            channel.kind,
            serenity::ChannelType::Text | serenity::ChannelType::News
        )
    {
        reply(ctx, "Pick a text channel for the poll.").await?;
        return Ok(());
    }
    let options: Vec<String> = [Some(option_1), Some(option_2), option_3, option_4, option_5]
        .into_iter()
        .flatten()
        .collect();
    // Evaluating the classes reads a lot of rows; answer within Discord's deadline first.
    ctx.defer_ephemeral().await?;
    let request = CreateRequest {
        title: question,
        options,
        requires,
        duration_seconds,
        channel_id,
        created_by: ctx.author().id,
    };
    match service.create(ctx.serenity_context(), &request).await {
        Ok(CreateOutcome::Created {
            poll_id,
            message,
            channel,
            eligible,
            warnings,
        }) => {
            let mut text = format!(
                "Poll #{poll_id} is live: https://discord.com/channels/{}/{channel}/{message}\nWho can vote was frozen now.",
                config::GUILD_ID
            );
            if let Some(eligible) = eligible {
                let _ = write!(text, " {eligible} eligible players.");
            }
            for warning in &warnings {
                let _ = write!(text, "\nWarning: {warning}");
            }
            reply(ctx, &text).await?;
        }
        Ok(CreateOutcome::Rejected(message)) => reply(ctx, &message).await?,
        Err(error) => {
            tracing::error!(%error, "/poll create failed");
            reply(
                ctx,
                "Creating the poll failed. Nothing was posted; check the bot logs.",
            )
            .await?;
        }
    }
    Ok(())
}

/// Close a poll now and show the final counts.
#[poise::command(slash_command, guild_only)]
async fn close(
    ctx: Context<'_>,
    #[description = "Poll number (shown in the poll footer)"] poll_id: u64,
) -> Result<(), Error> {
    let Some(service) = require_staff_and_service(ctx).await? else {
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    match service.close(ctx.serenity_context(), poll_id).await {
        Ok(CloseOutcome::Closed { text } | CloseOutcome::NotOpen(text)) => {
            reply(ctx, &text).await?;
        }
        Err(error) => {
            tracing::error!(%error, poll_id, "/poll close failed");
            reply(ctx, "Closing the poll failed; check the bot logs.").await?;
        }
    }
    Ok(())
}

/// Show the counts of a poll (eligible votes only).
#[poise::command(slash_command, guild_only)]
async fn results(
    ctx: Context<'_>,
    #[description = "Poll number (shown in the poll footer)"] poll_id: u64,
) -> Result<(), Error> {
    let Some(service) = require_staff_and_service(ctx).await? else {
        return Ok(());
    };
    match service.results(poll_id).await {
        Ok(Some(text)) => reply(ctx, &text).await?,
        Ok(None) => reply(ctx, "That poll does not exist.").await?,
        Err(error) => {
            tracing::error!(%error, poll_id, "/poll results failed");
            reply(ctx, "Reading the poll failed; check the bot logs.").await?;
        }
    }
    Ok(())
}

/// The same check as the other staff commands: administrators, Terminators,
/// Marketers and Developers.
async fn require_staff_and_service(ctx: Context<'_>) -> Result<Option<PollService>, Error> {
    let member = ctx
        .author_member()
        .await
        .context("missing command member")?;
    if !moderation::is_administrator(&member)
        && !moderation::has_any_role(&member, config::AUTHORIZED_ROLE_IDS)
    {
        reply(
            ctx,
            "You do not have permission to use this command. Required roles: Terminator, Marketer, or Dev.",
        )
        .await?;
        return Ok(None);
    }
    let Some(service) = ctx.data().polls.clone() else {
        reply(ctx, "Polls are not enabled on this bot.").await?;
        return Ok(None);
    };
    Ok(Some(service))
}

async fn reply(ctx: Context<'_>, text: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().ephemeral(true).content(text))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_poll_command_fits_discords_limits() {
        let commands = poise::builtins::create_application_commands(&[poll()]);
        let json = serde_json::to_value(&commands).unwrap();
        let parent = &json[0];
        assert_eq!(parent["name"], "poll");
        let subcommands = parent["options"].as_array().unwrap();
        let names: Vec<_> = subcommands
            .iter()
            .map(|sub| sub["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["create", "close", "results"]);
        for sub in subcommands {
            assert!(sub["description"].as_str().unwrap().chars().count() <= 100);
            let options = sub["options"].as_array().unwrap();
            assert!(options.len() <= 25);
            // Discord rejects a command that lists a required option after an optional one.
            let mut seen_optional = false;
            for option in options {
                assert!(option["description"].as_str().unwrap().chars().count() <= 100);
                let required = option["required"].as_bool().unwrap_or(false);
                assert!(!(seen_optional && required), "{option}");
                seen_optional |= !required;
            }
        }
        let create = &subcommands[0]["options"];
        assert_eq!(create.as_array().unwrap().len(), 9);
    }
}
