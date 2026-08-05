use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use poise::{CreateReply, serenity_prelude as serenity};

use crate::{
    config,
    media::{MAX_FREQUENCY, MIN_FREQUENCY},
    moderation::{self, ApprovalAction},
    state::{Context, Error},
};

/// Set the server banner with two-person approval.
#[poise::command(slash_command, guild_only)]
pub async fn discordbannerset(
    ctx: Context<'_>,
    #[description = "Upload a PNG, JPG, GIF, or WebP banner"] image: Option<serenity::Attachment>,
    #[description = "URL to a hosted banner image"] url: Option<String>,
) -> Result<(), Error> {
    let member = ctx
        .author_member()
        .await
        .context("missing command member")?;
    if !moderation::is_administrator(&member)
        && !moderation::has_any_role(&member, config::AUTHORIZED_ROLE_IDS)
    {
        deny(ctx, "You do not have permission to use this command. Required roles: Terminator, Marketer, or Dev.").await?;
        return Ok(());
    }
    let image_url = validate_banner(image.as_ref(), url.as_deref())?;
    let is_animated = image_url
        .split('?')
        .next()
        .is_some_and(|path| path.to_ascii_lowercase().ends_with(".gif"));
    let premium_tier = ctx.guild().map(|guild| guild.premium_tier);
    let required_tier = if is_animated {
        serenity::PremiumTier::Tier3
    } else {
        serenity::PremiumTier::Tier2
    };
    if premium_tier.is_some_and(|tier| tier < required_tier) {
        deny(
            ctx,
            if is_animated {
                "This server needs Boost Level 3 to set an animated banner."
            } else {
                "This server needs Boost Level 2 to set a banner."
            },
        )
        .await?;
        return Ok(());
    }
    let guild_id = ctx.guild_id().context("missing guild")?;
    if moderation::is_administrator(&member) {
        ctx.defer_ephemeral().await?;
        set_banner(ctx.http(), &ctx.data().http, guild_id, &image_url).await?;
        ctx.say("The server banner was updated with the administrator bypass.")
            .await?;
        log_action(
            &ctx,
            "Server Banner Changed",
            format!(
                "Submitted by <@{}> via administrator bypass.\n[View image]({image_url})",
                ctx.author().id
            ),
        )
        .await;
        return Ok(());
    }
    let request = ctx
        .data()
        .pending
        .create(
            ctx.author().id,
            ctx.author().tag(),
            guild_id,
            ApprovalAction::Banner {
                image_url: image_url.clone(),
            },
        )
        .await;
    send_vote(&ctx, "banner", &request, serenity::CreateEmbed::new().title("Banner Change Request").description(format!("<@{}> wants to change the server banner. A different Terminator must approve this request.\nExpires: <t:{}:R>", request.submitter_id, chrono::Utc::now().timestamp() + 3_600)).image(image_url)).await?;
    ctx.send(
        CreateReply::default()
            .ephemeral(true)
            .content("Your banner change request has been submitted for approval."),
    )
    .await?;
    Ok(())
}

/// Ban a user with two-person approval.
#[poise::command(slash_command, guild_only)]
pub async fn terminatorban(
    ctx: Context<'_>,
    #[description = "The user to ban"] user: serenity::User,
    #[description = "Reason for the ban"] reason: Option<String>,
    #[description = "Days of messages to delete (0-7)"]
    #[min = 0]
    #[max = 7]
    delete_messages: Option<u8>,
) -> Result<(), Error> {
    let member = ctx
        .author_member()
        .await
        .context("missing command member")?;
    if !moderation::is_administrator(&member)
        && !moderation::has_any_role(&member, config::AUTHORIZED_ROLE_IDS)
    {
        deny(ctx, "You do not have permission to use this command. Required roles: Terminator, Marketer, or Dev.").await?;
        return Ok(());
    }
    if user.id == ctx.author().id {
        deny(ctx, "You cannot ban yourself.").await?;
        return Ok(());
    }
    if user.id == ctx.framework().bot_id {
        deny(ctx, "I cannot ban myself.").await?;
        return Ok(());
    }
    let guild_id = ctx.guild_id().context("missing guild")?;
    let reason = reason.unwrap_or_else(|| "No reason provided".into());
    let delete_message_days = delete_messages.unwrap_or(0);
    if let Ok(target_member) = guild_id.member(ctx.http(), user.id).await {
        let roles = guild_id.roles(ctx.http()).await?;
        let target_position = highest_role_position(&target_member.roles, &roles);
        let requester_position = highest_role_position(&member.roles, &roles);
        if !moderation::is_administrator(&member) && target_position >= requester_position {
            deny(
                ctx,
                "You cannot ban someone with a higher or equal role than yours.",
            )
            .await?;
            return Ok(());
        }
        let bot_member = guild_id.member(ctx.http(), ctx.framework().bot_id).await?;
        if target_position >= highest_role_position(&bot_member.roles, &roles) {
            deny(
                ctx,
                "I cannot ban this user because their role is higher than or equal to mine.",
            )
            .await?;
            return Ok(());
        }
    }
    if moderation::is_administrator(&member) {
        ctx.defer_ephemeral().await?;
        guild_id
            .ban_with_reason(
                ctx.http(),
                user.id,
                delete_message_days,
                format!(
                    "Banned by {} (administrator bypass): {reason}",
                    ctx.author().tag()
                ),
            )
            .await?;
        ctx.say(format!(
            "{} was banned with the administrator bypass.",
            user.tag()
        ))
        .await?;
        log_action(&ctx, "User Banned", format!("Target: <@{}> ({})\nSubmitted by: <@{}>\nReason: {reason}\nApproved via administrator bypass", user.id, user.tag(), ctx.author().id)).await;
        return Ok(());
    }
    let request = ctx
        .data()
        .pending
        .create(
            ctx.author().id,
            ctx.author().tag(),
            guild_id,
            ApprovalAction::Ban {
                target_id: user.id,
                reason: reason.clone(),
                delete_message_days,
            },
        )
        .await;
    send_vote(&ctx, "ban", &request, serenity::CreateEmbed::new().title("Ban Request").description(format!("<@{}> wants to ban <@{}>. A different Terminator must approve this request.\nExpires: <t:{}:R>", request.submitter_id, user.id, chrono::Utc::now().timestamp() + 3_600)).field("Reason", reason, false).field("Messages to Delete", format!("{delete_message_days} day(s)"), true)).await?;
    ctx.send(CreateReply::default().ephemeral(true).content(format!(
        "Your ban request for {} has been submitted for approval.",
        user.tag()
    )))
    .await?;
    Ok(())
}

/// Change the Mini-Terminator role with two-person approval.
#[poise::command(slash_command, guild_only, subcommands("mini_add", "mini_remove"))]
#[allow(clippy::unused_async)]
pub async fn miniterminator(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Grant the Mini-Terminator role to a user.
#[poise::command(slash_command, guild_only, rename = "add")]
async fn mini_add(
    ctx: Context<'_>,
    #[description = "User to grant the role"] user: serenity::User,
) -> Result<(), Error> {
    mini_role(ctx, user, true).await
}

/// Remove the Mini-Terminator role from a user.
#[poise::command(slash_command, guild_only, rename = "remove")]
async fn mini_remove(
    ctx: Context<'_>,
    #[description = "User to remove the role"] user: serenity::User,
) -> Result<(), Error> {
    mini_role(ctx, user, false).await
}

async fn mini_role(ctx: Context<'_>, user: serenity::User, add: bool) -> Result<(), Error> {
    let member = ctx
        .author_member()
        .await
        .context("missing command member")?;
    if !moderation::is_administrator(&member)
        && !moderation::has_role(&member, config::TERMINATOR_ROLE_ID)
    {
        deny(
            ctx,
            "Only members with the Terminator role can use this command.",
        )
        .await?;
        return Ok(());
    }
    let guild_id = ctx.guild_id().context("missing guild")?;
    let target = guild_id
        .member(ctx.http(), user.id)
        .await
        .context("that user is not in the server")?;
    let has_role = target.roles.contains(&config::MINI_TERMINATOR_ROLE_ID);
    if add == has_role {
        deny(
            ctx,
            if add {
                "That user already has the Mini-Terminator role."
            } else {
                "That user does not have the Mini-Terminator role."
            },
        )
        .await?;
        return Ok(());
    }
    if moderation::is_administrator(&member) {
        ctx.defer_ephemeral().await?;
        apply_mini_role(ctx.http(), guild_id, user.id, add).await?;
        ctx.say(format!(
            "The Mini-Terminator role was {} with the administrator bypass.",
            if add { "granted" } else { "removed" }
        ))
        .await?;
        log_action(
            &ctx,
            "Mini-Terminator Role Changed",
            format!(
                "Target: <@{}>\nAction: {}\nSubmitted by: <@{}> via administrator bypass",
                user.id,
                if add { "add" } else { "remove" },
                ctx.author().id
            ),
        )
        .await;
        return Ok(());
    }
    let request = ctx
        .data()
        .pending
        .create(
            ctx.author().id,
            ctx.author().tag(),
            guild_id,
            ApprovalAction::MiniTerminator {
                target_id: user.id,
                add,
            },
        )
        .await;
    send_vote(&ctx, "mini", &request, serenity::CreateEmbed::new().title("Mini-Terminator Role Change").description(format!("<@{}> wants to {} the Mini-Terminator role {} <@{}>. A different Terminator must approve this request.\nExpires: <t:{}:R>", request.submitter_id, if add { "grant" } else { "remove" }, if add { "to" } else { "from" }, user.id, chrono::Utc::now().timestamp() + 3_600))).await?;
    ctx.send(
        CreateReply::default()
            .ephemeral(true)
            .content("Your Mini-Terminator role change has been submitted for approval."),
    )
    .await?;
    Ok(())
}

/// Change the media channel reminder frequency.
#[poise::command(slash_command, guild_only)]
pub async fn mediachannelsfreq(
    ctx: Context<'_>,
    #[description = "Messages between reminders"]
    #[min = 1]
    #[max = 100]
    number: u16,
) -> Result<(), Error> {
    let member = ctx
        .author_member()
        .await
        .context("missing command member")?;
    if !moderation::has_role(&member, config::TERMINATOR_ROLE_ID) {
        deny(
            ctx,
            "Only members with the Terminator role can change media channel frequency.",
        )
        .await?;
        return Ok(());
    }
    if !(MIN_FREQUENCY..=MAX_FREQUENCY).contains(&number) {
        bail!("frequency must be between {MIN_FREQUENCY} and {MAX_FREQUENCY}")
    }
    let current = ctx.data().media.frequency().await;
    if current == number {
        deny(
            ctx,
            &format!("Media channel reminders are already set to every {number} message(s)."),
        )
        .await?;
        return Ok(());
    }
    let guild_id = ctx.guild_id().context("missing guild")?;
    let request = ctx
        .data()
        .pending
        .create(
            ctx.author().id,
            ctx.author().tag(),
            guild_id,
            ApprovalAction::MediaFrequency { requested: number },
        )
        .await;
    send_vote(&ctx, "mediafreq", &request, serenity::CreateEmbed::new().title("Media Channel Frequency Change").description(format!("<@{}> wants to change the reminder frequency. A different Terminator must approve this request.\nExpires: <t:{}:R>", request.submitter_id, chrono::Utc::now().timestamp() + 3_600)).field("Current Frequency", current.to_string(), true).field("New Frequency", number.to_string(), true)).await?;
    ctx.send(
        CreateReply::default()
            .ephemeral(true)
            .content("Your frequency change has been submitted for approval."),
    )
    .await?;
    Ok(())
}

async fn deny(ctx: Context<'_>, message: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().ephemeral(true).content(message))
        .await?;
    Ok(())
}

fn highest_role_position(
    role_ids: &[serenity::RoleId],
    roles: &std::collections::HashMap<serenity::RoleId, serenity::Role>,
) -> u16 {
    role_ids
        .iter()
        .filter_map(|role_id| roles.get(role_id).map(|role| role.position))
        .max()
        .unwrap_or(0)
}

async fn send_vote(
    ctx: &Context<'_>,
    prefix: &str,
    request: &crate::moderation::ApprovalRequest,
    embed: serenity::CreateEmbed,
) -> Result<(), Error> {
    let channel_id = ctx
        .data()
        .environment
        .vote_channel_id
        .context("VOTE_CHANNEL_ID is not configured")?;
    channel_id
        .send_message(
            ctx.http(),
            serenity::CreateMessage::new()
                .content(format!(
                    "<@&{}> an approval is needed.",
                    config::TERMINATOR_ROLE_ID
                ))
                .allowed_mentions(
                    serenity::CreateAllowedMentions::new().roles([config::TERMINATOR_ROLE_ID]),
                )
                .embed(
                    embed
                        .colour(0x00FE_E75C)
                        .field("Status", "Awaiting confirmation", true)
                        .footer(serenity::CreateEmbedFooter::new(format!(
                            "Request ID: {}",
                            request.id
                        ))),
                )
                .components(vec![moderation::approval_buttons(
                    prefix, request.id, false,
                )]),
        )
        .await?;
    Ok(())
}

fn validate_banner(image: Option<&serenity::Attachment>, url: Option<&str>) -> Result<String> {
    if let Some(image) = image {
        let valid_type = image.content_type.as_deref().is_some_and(|kind| {
            matches!(
                kind.split(';').next(),
                Some("image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp")
            )
        });
        let valid_extension = [".png", ".jpg", ".jpeg", ".gif", ".webp"]
            .iter()
            .any(|extension| image.filename.to_ascii_lowercase().ends_with(extension));
        if !valid_type && !valid_extension {
            bail!("invalid image type; use PNG, JPG, GIF, or WebP")
        }
        if image.size > 10 * 1024 * 1024 {
            bail!("banner images may not exceed 10 MB")
        }
        return Ok(image.url.clone());
    }
    let url = url.context("provide either an image attachment or an image URL")?;
    let parsed = reqwest::Url::parse(url).context("invalid image URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("image URL must use HTTP or HTTPS")
    }
    let path = parsed.path().to_ascii_lowercase();
    if ![".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|extension| path.ends_with(extension))
    {
        bail!("image URL must end in PNG, JPG, GIF, or WebP")
    }
    Ok(url.to_owned())
}

pub async fn set_banner(
    http: &serenity::Http,
    client: &reqwest::Client,
    guild_id: serenity::GuildId,
    url: &str,
) -> Result<()> {
    let response = client.get(url).send().await?.error_for_status()?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("image/png")
        .split(';')
        .next()
        .unwrap_or("image/png")
        .to_owned();
    let bytes = response.bytes().await?;
    if bytes.len() > 10 * 1024 * 1024 {
        bail!("banner images may not exceed 10 MB")
    }
    let data = format!("data:{content_type};base64,{}", STANDARD.encode(bytes));
    guild_id
        .edit(http, serenity::EditGuild::new().banner(Some(data)))
        .await?;
    Ok(())
}

pub async fn apply_mini_role(
    http: &serenity::Http,
    guild_id: serenity::GuildId,
    target_id: serenity::UserId,
    add: bool,
) -> Result<()> {
    let member = guild_id.member(http, target_id).await?;
    if add {
        member
            .add_role(http, config::MINI_TERMINATOR_ROLE_ID)
            .await?;
    } else {
        member
            .remove_role(http, config::MINI_TERMINATOR_ROLE_ID)
            .await?;
    }
    Ok(())
}

pub async fn log_action(ctx: &Context<'_>, title: &str, description: String) {
    let Some(channel_id) = ctx.data().environment.log_channel_id else {
        return;
    };
    if let Err(error) = channel_id
        .send_message(
            ctx.http(),
            serenity::CreateMessage::new().embed(
                serenity::CreateEmbed::new()
                    .title(title)
                    .description(description)
                    .colour(0x002B_2D31),
            ),
        )
        .await
    {
        tracing::error!(%error, "failed to send moderation audit log");
    }
}
