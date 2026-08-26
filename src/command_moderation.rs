use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use poise::{CreateReply, serenity_prelude as serenity};

use crate::{
    config,
    media::{MAX_FREQUENCY, MIN_FREQUENCY},
    moderation::{self, ApprovalAction, BannerLocation},
    state::{Context, Error},
};

/// Set the server banner, invite splash, or Discovery splash with two-person approval.
#[poise::command(slash_command, guild_only)]
pub async fn discordbannerset(
    ctx: Context<'_>,
    #[description = "Upload a PNG, JPG, GIF, or WebP banner"] image: Option<serenity::Attachment>,
    #[description = "URL to a hosted banner image"] url: Option<String>,
    #[description = "Where to set the image; defaults to the server banner"] location: Option<
        BannerLocation,
    >,
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
    let banner = validate_banner(image.as_ref(), url.as_deref())?;
    let location = location.unwrap_or(BannerLocation::Banner);
    let image_url = banner.url;
    let premium_tier = ctx.guild().map(|guild| guild.premium_tier);
    if let Some((required, (name, level))) = boost_gate(location, banner.is_animated)
        && premium_tier.is_some_and(|tier| tier < required)
    {
        deny(
            ctx,
            &format!("This server needs Boost {level} to set {name}."),
        )
        .await?;
        return Ok(());
    }
    if location == BannerLocation::DiscoverySplash
        && !ctx.guild().is_some_and(|guild| {
            guild
                .features
                .iter()
                .any(|feature| feature == "DISCOVERABLE")
        })
    {
        deny(
            ctx,
            "This server is not in the Server Discovery directory, so the Discovery splash cannot be set.",
        )
        .await?;
        return Ok(());
    }
    let guild_id = ctx.guild_id().context("missing guild")?;
    if moderation::is_administrator(&member) {
        ctx.defer_ephemeral().await?;
        set_guild_image(ctx.http(), &ctx.data().http, guild_id, location, &image_url).await?;
        ctx.say(format!(
            "The {} was updated with the administrator bypass.",
            location.label()
        ))
        .await?;
        log_action(
            &ctx,
            location.change_title(),
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
            ApprovalAction::GuildImage {
                image_url: image_url.clone(),
                location,
            },
        )
        .await;
    let request_title = match location {
        BannerLocation::Banner => "Banner Change Request",
        BannerLocation::Splash => "Invite Splash Change Request",
        BannerLocation::DiscoverySplash => "Discovery Splash Change Request",
    };
    send_vote(&ctx, "banner", &request, serenity::CreateEmbed::new().title(request_title).description(format!("<@{}> wants to change the {}. A different Terminator must approve this request.\nExpires: <t:{}:R>", request.submitter_id, location.label(), chrono::Utc::now().timestamp() + 3_600)).image(image_url)).await?;
    ctx.send(CreateReply::default().ephemeral(true).content(format!(
        "Your {} change request has been submitted for approval.",
        location.label()
    )))
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
    let guild = guild_id.to_partial_guild(ctx.http()).await?;
    if guild.owner_id == user.id {
        deny(ctx, "The server owner cannot be banned.").await?;
        return Ok(());
    }
    let bot_member = guild_id.member(ctx.http(), ctx.framework().bot_id).await?;
    if !guild.member_permissions(&bot_member).ban_members() {
        deny(
            ctx,
            "I cannot ban members because I do not have the Ban Members permission.",
        )
        .await?;
        return Ok(());
    }
    let reason = reason.unwrap_or_else(|| "No reason provided".into());
    let delete_message_days = delete_messages.unwrap_or(0);
    if let Ok(target_member) = guild_id.member(ctx.http(), user.id).await {
        let target_position = highest_role_position(&target_member.roles, &guild.roles);
        let requester_position = highest_role_position(&member.roles, &guild.roles);
        if !moderation::is_administrator(&member) && target_position >= requester_position {
            deny(
                ctx,
                "You cannot ban someone with a higher or equal role than yours.",
            )
            .await?;
            return Ok(());
        }
        if target_position >= highest_role_position(&bot_member.roles, &guild.roles) {
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

/// Unban a user with two-person approval.
#[poise::command(slash_command, guild_only)]
pub async fn terminatorunban(
    ctx: Context<'_>,
    #[description = "The user to unban"] user: serenity::User,
    #[description = "Reason for the unban"] reason: Option<String>,
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
    let guild_id = ctx.guild_id().context("missing guild")?;
    if moderation::is_administrator(&member) {
        ctx.defer_ephemeral().await?;
        guild_id
            .unban(ctx.http(), user.id)
            .await
            .context("failed to unban user")?;
        ctx.say(format!(
            "{} was unbanned with the administrator bypass.",
            user.tag()
        ))
        .await?;
        log_action(&ctx, "User Unbanned", format!("Target: <@{}> ({})\nSubmitted by: <@{}>\nReason: {}\nApproved via administrator bypass", user.id, user.tag(), ctx.author().id, reason.as_deref().unwrap_or("No reason provided"))).await;
        return Ok(());
    }
    let reason = reason.unwrap_or_else(|| "No reason provided".into());
    let request = ctx
        .data()
        .pending
        .create(
            ctx.author().id,
            ctx.author().tag(),
            guild_id,
            ApprovalAction::Unban {
                target_id: user.id,
                reason: reason.clone(),
            },
        )
        .await;
    send_vote(&ctx, "unban", &request, serenity::CreateEmbed::new().title("Unban Request").description(format!("<@{}> wants to unban <@{}>. A different Terminator must approve this request.\nExpires: <t:{}:R>", request.submitter_id, user.id, chrono::Utc::now().timestamp() + 3_600)).field("Reason", reason, false)).await?;
    ctx.send(CreateReply::default().ephemeral(true).content(format!(
        "Your unban request for {} has been submitted for approval.",
        user.tag()
    )))
    .await?;
    Ok(())
}

/// Bulk delete recent messages to clean up spam.
#[poise::command(slash_command, guild_only)]
pub async fn purge(
    ctx: Context<'_>,
    #[description = "Number of messages to delete (1-1000)"]
    #[min = 1]
    #[max = 1000]
    amount: u16,
    #[description = "Only delete messages from this user"] user: Option<serenity::User>,
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
    ctx.defer_ephemeral().await?;
    let channel = ctx
        .channel_id()
        .to_channel(ctx)
        .await
        .context("missing channel")?
        .guild()
        .context("purge must be used in a guild text channel")?;
    let channel_id = channel.id;
    let bot_id = ctx.framework().bot_id;
    if let Some(permissions) = ctx.guild().and_then(|guild| {
        guild
            .members
            .get(&bot_id)
            .map(|member| guild.user_permissions_in(&channel, member))
    }) {
        let required = serenity::Permissions::VIEW_CHANNEL
            | serenity::Permissions::READ_MESSAGE_HISTORY
            | serenity::Permissions::MANAGE_MESSAGES;
        if !permissions.contains(required) {
            ctx.say("I cannot purge messages in this channel. Give the bot View Channel, Read Message History, and Manage Messages permissions here.")
                .await?;
            return Ok(());
        }
    }

    let target = user.map(|user| user.id);
    let selection = match collect_purge_messages(ctx.http(), channel_id, amount as usize, target)
        .await
    {
        Ok(selection) => selection,
        Err(error) => {
            tracing::error!(%error, %channel_id, "failed to read messages for purge");
            ctx.say(format!(
                "Purge failed while reading this channel: {error}. Check the bot's View Channel and Read Message History permissions."
            ))
            .await?;
            return Ok(());
        }
    };
    let collected = selection.fresh.len() + selection.old.len();
    let scanned = selection.scanned;
    let outcome = delete_purge_messages(ctx.http(), channel_id, &selection).await;
    let deleted = outcome.deleted;
    let failed = outcome.failed;

    let result = if failed > 0 {
        format!(
            "Purged {deleted} message(s) in <#{channel_id}>, but Discord rejected {failed}: {}",
            outcome
                .first_error
                .as_deref()
                .unwrap_or("unknown deletion error")
        )
    } else if deleted == 0 {
        if target.is_some() {
            format!(
                "No matching messages were found in the newest {scanned} message(s) in <#{channel_id}>."
            )
        } else {
            format!("No messages were available to purge in <#{channel_id}>.")
        }
    } else if deleted < amount as usize {
        format!(
            "Purged {deleted} matching message(s) in <#{channel_id}>; only {collected} were found after scanning {scanned}."
        )
    } else {
        format!("Purged {deleted} message(s) in <#{channel_id}>.")
    };
    ctx.say(result).await?;
    Ok(())
}

fn purge_scan_limit(amount: usize, filtered: bool) -> usize {
    if filtered {
        amount.saturating_mul(100).clamp(1_000, 10_000)
    } else {
        amount
    }
}

struct PurgeSelection {
    fresh: Vec<serenity::MessageId>,
    old: Vec<serenity::MessageId>,
    scanned: usize,
}

struct PurgeOutcome {
    deleted: usize,
    failed: usize,
    first_error: Option<String>,
}

async fn collect_purge_messages(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    amount: usize,
    target: Option<serenity::UserId>,
) -> serenity::Result<PurgeSelection> {
    let cutoff = chrono::Utc::now().timestamp() - 14 * 24 * 60 * 60;
    let scan_limit = purge_scan_limit(amount, target.is_some());
    let mut cursor = None;
    let mut fresh = Vec::new();
    let mut old = Vec::new();
    let mut scanned = 0usize;

    while fresh.len() + old.len() < amount && scanned < scan_limit {
        let page_size = u8::try_from((scan_limit - scanned).min(100))
            .expect("Discord message page size is capped at 100");
        let mut request = serenity::GetMessages::new().limit(page_size);
        if let Some(cursor) = cursor {
            request = request.before(cursor);
        }
        let messages = channel_id.messages(http, request).await?;
        if messages.is_empty() {
            break;
        }
        cursor = messages.last().map(|message| message.id);
        scanned += messages.len();
        for message in messages {
            if fresh.len() + old.len() >= amount {
                break;
            }
            if target.is_some_and(|id| message.author.id != id) {
                continue;
            }
            if message.id.created_at().unix_timestamp() >= cutoff {
                fresh.push(message.id);
            } else {
                old.push(message.id);
            }
        }
    }
    Ok(PurgeSelection {
        fresh,
        old,
        scanned,
    })
}

async fn delete_purge_messages(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    selection: &PurgeSelection,
) -> PurgeOutcome {
    let mut outcome = PurgeOutcome {
        deleted: 0,
        failed: 0,
        first_error: None,
    };
    for chunk in selection.fresh.chunks(100) {
        if let Err(error) = channel_id.delete_messages(http, chunk).await {
            tracing::warn!(%error, %channel_id, messages = chunk.len(), "bulk purge failed; retrying messages individually");
            for id in chunk {
                record_purge_deletion(
                    &mut outcome,
                    channel_id,
                    *id,
                    channel_id.delete_message(http, *id).await,
                );
            }
        } else {
            outcome.deleted += chunk.len();
        }
    }
    for id in &selection.old {
        record_purge_deletion(
            &mut outcome,
            channel_id,
            *id,
            channel_id.delete_message(http, *id).await,
        );
    }
    outcome
}

fn record_purge_deletion(
    outcome: &mut PurgeOutcome,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    result: serenity::Result<()>,
) {
    if let Err(error) = result {
        tracing::error!(%error, %channel_id, %message_id, "failed to delete message during purge");
        outcome.failed += 1;
        outcome.first_error.get_or_insert_with(|| error.to_string());
    } else {
        outcome.deleted += 1;
    }
}

static LANGUAGE_REPAIR_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
    std::sync::OnceLock::new();

/// Reapply language roles from the reactions on every language menu.
#[poise::command(slash_command, guild_only)]
pub async fn reapplylanguages(ctx: Context<'_>) -> Result<(), Error> {
    let member = ctx
        .author_member()
        .await
        .context("missing command member")?;
    if !moderation::is_administrator(&member)
        && !moderation::has_role(&member, config::DEVELOPER_ROLE_ID)
    {
        deny(
            ctx,
            "Only administrators or members with the Developer role can reapply language roles.",
        )
        .await?;
        return Ok(());
    }
    let Ok(_guard) = LANGUAGE_REPAIR_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .try_lock()
    else {
        deny(ctx, "A language-role repair is already running.").await?;
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let reply = ctx
        .send(
            CreateReply::default()
                .ephemeral(true)
                .content("Scanning all language-menu reactions and reapplying missing roles…"),
        )
        .await?;
    let result = repair_language_roles(ctx.serenity_context()).await;
    let summary = match result {
        Ok(stats) => stats.summary(),
        Err(error) => {
            tracing::error!(%error, "language-role repair failed");
            format!("Language-role repair failed: {error}")
        }
    };
    log_action(&ctx, "Language Roles Reapplied", summary.clone()).await;
    if let Err(error) = reply
        .edit(ctx, CreateReply::default().ephemeral(true).content(summary))
        .await
    {
        tracing::warn!(%error, "failed to update language-role repair response");
    }
    Ok(())
}

#[derive(Default)]
struct LanguageRepairStats {
    menus: usize,
    users: usize,
    assignments: usize,
    added: usize,
    already_present: usize,
    failed: usize,
}

impl LanguageRepairStats {
    fn summary(&self) -> String {
        format!(
            "Language-role repair complete: synced {} previously missing role assignment(s). Scanned {} menu(s), found {} user(s) and {} unique reaction assignment(s); {} were already synced and {} failed.",
            self.added, self.menus, self.users, self.assignments, self.already_present, self.failed
        )
    }
}

async fn repair_language_roles(ctx: &serenity::Context) -> Result<LanguageRepairStats> {
    let menus = language_menu_messages(ctx).await?;
    if menus.is_empty() {
        bail!("no bot-authored language menu was found");
    }
    let mut desired: HashMap<serenity::UserId, HashSet<serenity::RoleId>> = HashMap::new();
    for message in &menus {
        for (emoji, role_id) in &config::REACTION_ROLES[6..12] {
            collect_reaction_users(ctx, message, emoji, *role_id, &mut desired).await?;
        }
    }
    let mut stats = LanguageRepairStats {
        menus: menus.len(),
        users: desired.len(),
        assignments: desired.values().map(HashSet::len).sum(),
        ..Default::default()
    };
    for (user_id, roles) in desired {
        let member = match cached_or_fetched_member(ctx, user_id).await {
            Ok(member) => member,
            Err(error) => {
                stats.failed += roles.len();
                tracing::warn!(%error, %user_id, "could not load language-role member");
                continue;
            }
        };
        for role_id in roles {
            if member.roles.contains(&role_id) {
                stats.already_present += 1;
            } else if let Err(error) = member.add_role(ctx, role_id).await {
                stats.failed += 1;
                tracing::warn!(%error, %user_id, %role_id, "failed to reapply language role");
            } else {
                stats.added += 1;
            }
        }
    }
    Ok(stats)
}

async fn language_menu_messages(ctx: &serenity::Context) -> Result<Vec<serenity::Message>> {
    let mut menus = Vec::new();
    let mut before = None;
    loop {
        let mut request = serenity::GetMessages::new().limit(100);
        if let Some(message_id) = before {
            request = request.before(message_id);
        }
        let page = config::REACTION_ROLE_MENU_ID.messages(ctx, request).await?;
        if page.is_empty() {
            break;
        }
        before = page.last().map(|message| message.id);
        menus.extend(
            page.iter()
                .filter(|message| {
                    message.author.id == ctx.cache.current_user().id
                        && message.embeds.iter().any(|embed| {
                            embed.description.as_deref() == Some("Select your language.")
                        })
                })
                .cloned(),
        );
        if page.len() < 100 {
            break;
        }
    }
    Ok(menus)
}

async fn collect_reaction_users(
    ctx: &serenity::Context,
    message: &serenity::Message,
    emoji: &str,
    role_id: serenity::RoleId,
    desired: &mut HashMap<serenity::UserId, HashSet<serenity::RoleId>>,
) -> Result<()> {
    let mut after = None;
    loop {
        let users = message
            .reaction_users(
                ctx,
                serenity::ReactionType::Unicode(emoji.to_owned()),
                Some(100),
                after,
            )
            .await?;
        if users.is_empty() {
            break;
        }
        after = users.last().map(|user| user.id);
        let page_is_full = users.len() == 100;
        for user in users {
            if !user.bot {
                desired.entry(user.id).or_default().insert(role_id);
            }
        }
        if !page_is_full {
            break;
        }
    }
    Ok(())
}

async fn cached_or_fetched_member(
    ctx: &serenity::Context,
    user_id: serenity::UserId,
) -> serenity::Result<serenity::Member> {
    if let Some(member) = ctx
        .cache
        .guild(config::GUILD_ID)
        .and_then(|guild| guild.members.get(&user_id).cloned())
    {
        return Ok(member);
    }
    config::GUILD_ID.member(ctx, user_id).await
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
            ApprovalAction::MediaFrequency {
                current,
                requested: number,
            },
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

struct ValidatedBanner {
    url: String,
    is_animated: bool,
}

fn validate_banner(
    image: Option<&serenity::Attachment>,
    url: Option<&str>,
) -> Result<ValidatedBanner> {
    if let Some(image) = image {
        let filename = image.filename.to_ascii_lowercase();
        let content_type = image
            .content_type
            .as_deref()
            .and_then(|kind| kind.split(';').next());
        let valid_extension = has_image_extension(&filename);
        let valid_type = content_type.is_some_and(is_supported_image_type);
        if content_type.map_or(!valid_extension, |_| !valid_type) {
            bail!("invalid image type; use PNG, JPG, GIF, or WebP")
        }
        if image.size > 10 * 1024 * 1024 {
            bail!("banner images may not exceed 10 MB")
        }
        return Ok(ValidatedBanner {
            url: image.url.clone(),
            is_animated: content_type == Some("image/gif") || is_gif_filename(&filename),
        });
    }
    let url = url.context("provide either an image attachment or an image URL")?;
    let parsed = reqwest::Url::parse(url).context("invalid image URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("image URL must use HTTP or HTTPS")
    }
    let path = parsed.path().to_ascii_lowercase();
    if !has_image_extension(&path) {
        bail!("image URL must end in PNG, JPG, GIF, or WebP")
    }
    Ok(ValidatedBanner {
        url: url.to_owned(),
        is_animated: is_gif_filename(&path),
    })
}

fn is_supported_image_type(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    )
}

fn has_image_extension(filename: &str) -> bool {
    [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|extension| filename.ends_with(extension))
}

fn is_gif_filename(filename: &str) -> bool {
    std::path::Path::new(filename)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
}

pub async fn set_guild_image(
    http: &serenity::Http,
    client: &reqwest::Client,
    guild_id: serenity::GuildId,
    location: BannerLocation,
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
    let edit = match location {
        BannerLocation::Banner => serenity::EditGuild::new().banner(Some(data)),
        BannerLocation::Splash => serenity::EditGuild::new().splash(Some(data)),
        BannerLocation::DiscoverySplash => serenity::EditGuild::new().discovery_splash(Some(data)),
    };
    guild_id.edit(http, edit).await?;
    Ok(())
}

/// Boost tier required to set the image at `location`, plus the fragments for
/// the denial message. The Discovery splash has no boost requirement (it only
/// needs the server to be in Server Discovery), so it returns `None`.
fn boost_gate(
    location: BannerLocation,
    is_animated: bool,
) -> Option<(serenity::PremiumTier, (&'static str, &'static str))> {
    match (location, is_animated) {
        (BannerLocation::Banner, false) => {
            Some((serenity::PremiumTier::Tier2, ("a banner", "Level 2")))
        }
        (BannerLocation::Banner, true) => Some((
            serenity::PremiumTier::Tier3,
            ("an animated banner", "Level 3"),
        )),
        (BannerLocation::Splash, false) => Some((
            serenity::PremiumTier::Tier1,
            ("an invite splash", "Level 1"),
        )),
        (BannerLocation::Splash, true) => Some((
            serenity::PremiumTier::Tier3,
            ("an animated invite splash", "Level 3"),
        )),
        (BannerLocation::DiscoverySplash, _) => None,
    }
}

pub async fn apply_mini_role(
    http: &serenity::Http,
    guild_id: serenity::GuildId,
    target_id: serenity::UserId,
    add: bool,
) -> Result<()> {
    let member = match guild_id.member(http, target_id).await {
        Ok(member) => member,
        Err(error) if !add => {
            tracing::info!(%error, %target_id, "Mini-Terminator removal target is no longer in the server");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
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

#[cfg(test)]
mod tests {
    use super::{boost_gate, has_image_extension, is_supported_image_type, purge_scan_limit};
    use crate::moderation::BannerLocation;

    #[test]
    fn banner_types_require_supported_mime_when_present() {
        assert!(is_supported_image_type("image/gif"));
        assert!(!is_supported_image_type("text/plain"));
        assert!(has_image_extension("banner.webp"));
    }

    #[test]
    fn boost_gate_matches_discord_requirements() {
        use poise::serenity_prelude::PremiumTier as Tier;
        assert_eq!(
            boost_gate(BannerLocation::Banner, false).map(|(tier, _)| tier),
            Some(Tier::Tier2)
        );
        assert_eq!(
            boost_gate(BannerLocation::Banner, true).map(|(tier, _)| tier),
            Some(Tier::Tier3)
        );
        assert_eq!(
            boost_gate(BannerLocation::Splash, false).map(|(tier, _)| tier),
            Some(Tier::Tier1)
        );
        assert_eq!(
            boost_gate(BannerLocation::Splash, true).map(|(tier, _)| tier),
            Some(Tier::Tier3)
        );
        assert_eq!(boost_gate(BannerLocation::DiscoverySplash, false), None);
        assert_eq!(boost_gate(BannerLocation::DiscoverySplash, true), None);
    }

    #[test]
    fn filtered_purges_search_beyond_the_immediate_messages() {
        assert_eq!(purge_scan_limit(2, false), 2);
        assert_eq!(purge_scan_limit(2, true), 1_000);
        assert_eq!(purge_scan_limit(1_000, true), 10_000);
    }
}
