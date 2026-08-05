use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::{config, state::AppState};

pub async fn create(
    ctx: &serenity::Context,
    data: &AppState,
    message: &serenity::Message,
) -> Result<()> {
    if let Some(telegram) = &data.telegram
        && let Err(error) = telegram.message_create(message).await
    {
        tracing::error!(%error, message_id = %message.id, "Telegram create crosspost failed");
    }
    if message.author.bot || message.guild_id.is_none() {
        return Ok(());
    }
    if message.channel_id == config::UPDATES_ID
        && let Err(error) = message
            .author
            .direct_message(
                ctx,
                serenity::CreateMessage::new()
                    .content("Please post your announcement on r/6b6t subreddit too."),
            )
            .await
    {
        tracing::warn!(%error, user_id = %message.author.id, "failed to send updates reminder DM");
    }
    if message.channel_id == config::ADVERTISING_ID {
        replace_bot_reminder(ctx, message.channel_id, config::ADVERTISING_MESSAGE, None).await?;
    } else if message.channel_id == config::MERCH_ID {
        replace_bot_reminder(ctx, message.channel_id, config::MERCH_MESSAGE, None).await?;
    } else if is_media_channel(ctx, message).await
        && data.media.should_remind(message.channel_id).await
    {
        replace_bot_reminder(
            ctx,
            message.channel_id,
            config::MEDIA_CHANNEL_MESSAGE,
            Some(config::MEDIA_CHANNEL_MESSAGE),
        )
        .await?;
    }
    Ok(())
}

pub async fn update(
    ctx: &serenity::Context,
    data: &AppState,
    message: &serenity::Message,
) -> Result<()> {
    if let Some(telegram) = &data.telegram
        && let Err(error) = telegram.message_update(message).await
    {
        tracing::error!(%error, message_id = %message.id, "Telegram update crosspost failed");
    }
    if message.channel_id != config::REVIEW_ID || message.author.bot {
        return Ok(());
    }
    let Some(member) = &message.member else {
        return Ok(());
    };
    if config::REVIEW_IGNORE_ROLE_IDS
        .iter()
        .any(|role| member.roles.contains(role))
    {
        return Ok(());
    }
    message
        .delete(ctx)
        .await
        .context("failed to delete edited review message")?;
    Ok(())
}

pub async fn reaction(
    ctx: &serenity::Context,
    reaction: &serenity::Reaction,
    add: bool,
) -> Result<()> {
    let Some(user_id) = reaction.user_id else {
        return Ok(());
    };
    if user_id == ctx.cache.current_user().id {
        return Ok(());
    }
    let emoji = reaction.emoji.as_data();
    let Some((_, role_id)) = config::REACTION_ROLES
        .iter()
        .find(|(configured, _)| *configured == emoji)
    else {
        return Ok(());
    };
    let guild_id = reaction
        .guild_id
        .context("reaction role event was not in a guild")?;
    let member = guild_id.member(ctx, user_id).await?;
    if add {
        member.add_role(ctx, *role_id).await?;
    } else {
        member.remove_role(ctx, *role_id).await?;
    }
    Ok(())
}

async fn is_media_channel(ctx: &serenity::Context, message: &serenity::Message) -> bool {
    let Ok(channel) = message.channel_id.to_channel(ctx).await else {
        return false;
    };
    let serenity::Channel::Guild(channel) = channel else {
        return false;
    };
    let normalized = normalize_channel_name(&channel.name);
    config::MEDIA_CHANNEL_NAMES
        .iter()
        .any(|name| normalize_channel_name(name) == normalized)
}

fn normalize_channel_name(name: &str) -> String {
    name.to_ascii_lowercase()
        .trim_start_matches(|character: char| !character.is_ascii_alphanumeric())
        .to_owned()
}

async fn replace_bot_reminder(
    ctx: &serenity::Context,
    channel_id: serenity::ChannelId,
    content: &str,
    exact_content: Option<&str>,
) -> Result<()> {
    let messages = channel_id
        .messages(ctx, serenity::GetMessages::new().limit(100))
        .await?;
    if let Some(message) = messages.iter().find(|message| {
        message.author.id == ctx.cache.current_user().id
            && exact_content.is_none_or(|expected| message.content == expected)
    }) && let Err(error) = message.delete(ctx).await
    {
        tracing::warn!(%error, message_id = %message.id, "failed to delete old channel reminder");
    }
    channel_id
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .content(content)
                .allowed_mentions(serenity::CreateAllowedMentions::new())
                .flags(serenity::MessageFlags::SUPPRESS_NOTIFICATIONS),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_channel_name;
    #[test]
    fn channel_names_ignore_decorative_prefixes() {
        assert_eq!(normalize_channel_name("📷-Screenshots"), "screenshots");
    }
}
