use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::{config, state::AppState};

pub async fn create(
    ctx: &serenity::Context,
    data: &AppState,
    message: &serenity::Message,
) -> Result<()> {
    if let Some(telegram) = &data.telegram {
        telegram.queue_message_create(message.clone()).await;
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

pub async fn update_event(
    ctx: &serenity::Context,
    data: &AppState,
    event: &serenity::MessageUpdateEvent,
    cached: Option<&serenity::Message>,
) -> Result<()> {
    let fetched;
    let message = if let Some(message) = cached {
        message
    } else {
        fetched = event
            .channel_id
            .message(ctx, event.id)
            .await
            .context("failed to fetch updated message")?;
        &fetched
    };
    if let Some(telegram) = &data.telegram {
        telegram.queue_message_update(message.clone()).await;
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
    if reaction.channel_id != config::REACTION_ROLE_MENU_ID {
        return Ok(());
    }
    let Some(role_id) = reaction_role(&reaction.emoji) else {
        return Ok(());
    };
    let guild_id = reaction
        .guild_id
        .context("reaction role event was not in a guild")?;
    let member = guild_id.member(ctx, user_id).await?;
    if member.user.bot {
        return Ok(());
    }
    let result = if add {
        member.add_role(ctx, role_id).await
    } else {
        member.remove_role(ctx, role_id).await
    };
    if let Err(error) = result {
        tracing::warn!(
            %error,
            emoji = %reaction.emoji,
            %role_id,
            %user_id,
            action = if add { "add" } else { "remove" },
            "failed to update reaction role"
        );
    }
    Ok(())
}

fn reaction_role(emoji: &serenity::ReactionType) -> Option<serenity::RoleId> {
    config::REACTION_ROLES
        .iter()
        .find_map(|(configured, role_id)| emoji.unicode_eq(configured).then_some(*role_id))
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
    use super::{normalize_channel_name, reaction_role};
    use crate::config;
    use poise::serenity_prelude as serenity;

    #[test]
    fn channel_names_ignore_decorative_prefixes() {
        assert_eq!(normalize_channel_name("📷-Screenshots"), "screenshots");
    }

    #[test]
    fn reaction_roles_match_the_working_configuration() {
        let expected = [
            ("✨", 942_861_111_089_324_142),
            ("⚔️", 942_860_042_871_402_567),
            ("🌩️", 942_858_847_058_555_000),
            ("🎉", 1_155_462_541_871_415_326),
            ("🏄", 1_389_335_267_394_982_040),
            ("🎥", 1_423_961_521_997_746_227),
            ("🇺🇸", 1_051_075_005_250_809_966),
            ("🇷🇺", 1_072_504_173_637_144_636),
            ("🇪🇸", 1_051_075_060_238_123_078),
            ("🇹🇷", 1_330_608_186_436_096_023),
            ("🇩🇪", 1_325_150_138_997_543_047),
            ("🇵🇱", 1_121_818_071_384_981_607),
            ("🇫🇷", 1_544_778_354_652_086_322),
            ("🎮", 1_461_432_694_041_739_388),
        ];

        assert_eq!(config::REACTION_ROLES.len(), expected.len());
        for (emoji, role_id) in expected {
            assert_eq!(
                reaction_role(&serenity::ReactionType::Unicode(emoji.to_owned())),
                Some(serenity::RoleId::new(role_id))
            );
        }
    }
}
