use std::{collections::HashSet, future::Future, time::Duration};

use anyhow::Result;
use chrono::{Datelike as _, Timelike as _};
use chrono_tz::Europe::Berlin;
use futures::TryStreamExt as _;
use poise::serenity_prelude as serenity;

use crate::{config, state::AppState};

use super::role_sync;

pub async fn handle(
    ctx: &serenity::Context,
    data: &AppState,
    ready: &serenity::Ready,
) -> Result<()> {
    tracing::info!(user = %ready.user.tag(), guilds = ready.guilds.len(), "Discord bot connected");
    let mut started = data.ready_started.lock().await;
    if *started {
        return Ok(());
    }
    *started = true;
    drop(started);

    if let Some(telegram) = &data.telegram {
        let ctx = ctx.clone();
        let telegram = telegram.clone();
        spawn_startup_retry("Telegram initialization", move || {
            let ctx = ctx.clone();
            let telegram = telegram.clone();
            async move { telegram.ready(&ctx).await }
        });
    }
    let role_menu_ctx = ctx.clone();
    spawn_startup_retry("role menu initialization", move || {
        let ctx = role_menu_ctx.clone();
        async move { ensure_role_menu(&ctx).await }
    });
    let reaction_menu_ctx = ctx.clone();
    spawn_startup_retry("reaction menu initialization", move || {
        let ctx = reaction_menu_ctx.clone();
        async move { ensure_reaction_menus(&ctx).await }
    });

    spawn_interval(
        ctx.clone(),
        data.clone(),
        Duration::from_mins(5),
        |ctx, data| Box::pin(update_status(ctx, data)),
    );
    spawn_interval(
        ctx.clone(),
        data.clone(),
        Duration::from_mins(5),
        |ctx, data| Box::pin(clean_role_menu_roles(ctx, data)),
    );
    spawn_interval(
        ctx.clone(),
        data.clone(),
        Duration::from_mins(20),
        |ctx, data| Box::pin(youtube_notification(ctx, data)),
    );
    spawn_interval(
        ctx.clone(),
        data.clone(),
        Duration::from_secs(30),
        |ctx, data| Box::pin(role_sync::run(ctx, data)),
    );
    spawn_interval(
        ctx.clone(),
        data.clone(),
        Duration::from_mins(10),
        |_ctx, data| Box::pin(clean_pending_approvals(data)),
    );
    if data.anarchy.is_some() {
        spawn_interval(
            ctx.clone(),
            data.clone(),
            Duration::from_hours(1),
            |ctx, data| Box::pin(anarchy_analytics(ctx, data)),
        );
    }
    if data.community_event.is_some() {
        spawn_interval(
            ctx.clone(),
            data.clone(),
            Duration::from_secs(5),
            |ctx, data| Box::pin(community_event_announcements(ctx, data)),
        );
    }
    spawn_reminders(ctx.clone());
    Ok(())
}

fn spawn_startup_retry<F, Fut>(label: &'static str, task: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            match task().await {
                Ok(()) => return,
                Err(error) => {
                    tracing::error!(%error, task = label, "startup task failed; retrying");
                    tokio::time::sleep(Duration::from_mins(1)).await;
                }
            }
        }
    });
}

fn spawn_interval<F>(ctx: serenity::Context, data: AppState, interval: Duration, task: F)
where
    F: for<'a> Fn(&'a serenity::Context, &'a AppState) -> futures::future::BoxFuture<'a, ()>
        + Send
        + Sync
        + 'static,
{
    tokio::spawn(async move {
        task(&ctx, &data).await;
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            task(&ctx, &data).await;
        }
    });
}

fn spawn_reminders(ctx: serenity::Context) {
    tokio::spawn(async move {
        let mut sent = HashSet::new();
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        loop {
            ticker.tick().await;
            let now = chrono::Utc::now().with_timezone(&Berlin);
            if now.minute() != 0 || !matches!(now.hour(), 10 | 18) {
                continue;
            }
            let key = (now.year(), now.ordinal(), now.hour());
            if !sent.insert(key) {
                continue;
            }
            sent.retain(|(year, ordinal, _)| *year == now.year() && *ordinal + 2 >= now.ordinal());
            if let Err(error) = config::GENERAL_ID.say(&ctx, config::GENERAL_MESSAGE).await {
                tracing::error!(%error, "failed to send general rank reminder");
            }
        }
    });
}

async fn update_status(ctx: &serenity::Context, data: &AppState) {
    match data.server.server_data().await {
        Ok(server) => ctx.set_activity(Some(serenity::ActivityData::playing(format!(
            "IP: play.6b6t.org - Join {} other players online!",
            server.player_count
        )))),
        Err(error) => tracing::error!(%error, "failed to update Discord presence"),
    }
}

async fn youtube_notification(ctx: &serenity::Context, data: &AppState) {
    if let Err(error) = data.youtube.notify(ctx, config::YOUTUBE_ID).await {
        tracing::error!(%error, "YouTube notification check failed");
    }
}

async fn clean_pending_approvals(data: &AppState) {
    let removed = data.pending.cleanup_expired().await;
    if removed > 0 {
        tracing::info!(removed, "expired pending approval requests");
    }
}

async fn anarchy_analytics(ctx: &serenity::Context, data: &AppState) {
    let Some(anarchy) = &data.anarchy else {
        return;
    };
    let (online_users, online_players) = tokio::join!(
        data.server.anarchymod_player_count(),
        data.server.player_count(),
    );
    let online_users = match online_users {
        Ok(count) => Some(count),
        Err(error) => {
            tracing::warn!(%error, "failed to fetch the online AnarchyMod player count; the count is shown as unavailable");
            None
        }
    };
    let online_players = match online_players {
        Ok(count) => Some(count),
        Err(error) => {
            tracing::warn!(%error, "failed to fetch the player count for anarchy analytics; the online percentage is omitted");
            None
        }
    };
    if let Err(error) = anarchy.report(ctx, online_users, online_players).await {
        tracing::error!(%error, "anarchy mod analytics report failed");
    }
}

async fn community_event_announcements(ctx: &serenity::Context, data: &AppState) {
    let Some(service) = &data.community_event else {
        return;
    };
    if let Err(error) = service.poll(ctx).await {
        tracing::error!(%error, "community-event Discord announcement check failed");
    }
}

async fn clean_role_menu_roles(ctx: &serenity::Context, _data: &AppState) {
    let members = match config::GUILD_ID
        .members_iter(&ctx.http)
        .try_collect::<Vec<_>>()
        .await
    {
        Ok(members) => members,
        Err(error) => {
            tracing::error!(%error, "failed to load members for color role cleanup");
            return;
        }
    };
    for member in members {
        let has_menu_role = member
            .roles
            .iter()
            .any(|role| config::ROLE_MENU_ROLE_IDS.contains(role));
        let is_administrator = ctx.cache.guild(config::GUILD_ID).is_some_and(|guild| {
            guild.owner_id == member.user.id
                || member.roles.iter().any(|role_id| {
                    guild
                        .roles
                        .get(role_id)
                        .is_some_and(|role| role.permissions.administrator())
                })
        });
        let has_access =
            member.roles.contains(&config::ROLE_MENU_REQUIRED_ROLE_ID) || is_administrator;
        if has_menu_role && !has_access {
            for role in config::ROLE_MENU_ROLE_IDS {
                if member.roles.contains(role)
                    && let Err(error) = member.remove_role(ctx, *role).await
                {
                    tracing::error!(%error, member_id = %member.user.id, role_id = %role, "failed to remove inaccessible color role");
                }
            }
        }
    }
}

async fn ensure_role_menu(ctx: &serenity::Context) -> Result<()> {
    let messages = config::ROLE_MENU_ID
        .messages(ctx, serenity::GetMessages::new().limit(10))
        .await?;
    let exists = messages.iter().flat_map(|message| &message.components).flat_map(|row| &row.components).any(|component| matches!(component, serenity::ActionRowComponent::SelectMenu(menu) if menu.custom_id.as_deref() == Some("legend_role_menu")));
    if exists {
        return Ok(());
    }
    let roles = config::GUILD_ID.roles(ctx).await?;
    let mut options = vec![serenity::CreateSelectMenuOption::new("Clear", "clear_top")];
    options.extend(
        config::ROLE_MENU_ROLE_IDS
            .iter()
            .filter_map(|id| roles.get(id))
            .map(|role| serenity::CreateSelectMenuOption::new(&role.name, role.id.to_string())),
    );
    options.push(serenity::CreateSelectMenuOption::new(
        "Clear",
        "clear_bottom",
    ));
    let menu = serenity::CreateSelectMenu::new(
        "legend_role_menu",
        serenity::CreateSelectMenuKind::String { options },
    )
    .placeholder("Select a color");
    let embed = serenity::CreateEmbed::new()
        .title("Legend Color Roles")
        .description("Change your color in the Discord by picking one of the colors.")
        .image("https://www.6b6t.org/media/legend-color.gif")
        .thumbnail("https://www.6b6t.org/logo.png")
        .colour(0x00FF_F11A);
    config::ROLE_MENU_ID
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .embed(embed)
                .components(vec![serenity::CreateActionRow::SelectMenu(menu)]),
        )
        .await?;
    Ok(())
}

async fn ensure_reaction_menus(ctx: &serenity::Context) -> Result<()> {
    let messages = config::REACTION_ROLE_MENU_ID
        .messages(ctx, serenity::GetMessages::new().limit(100))
        .await?;
    let existing = messages
        .iter()
        .filter(|message| message.author.id == ctx.cache.current_user().id)
        .count();
    if existing >= 3 {
        return Ok(());
    }
    let menus = [
        (
            "Select your language.",
            0x0007_CFFA,
            &config::REACTION_ROLES[6..12],
            None,
        ),
        (
            "Select your notifications.\n\n✨ - General changes to 6b6t\n⚔️ - Crystal PvP, anticheat changes, PvP events and more\n🌩️ - Server going offline, online or restarting\n🎉 - Events and competitions in Discord and Minecraft\n🏄 - Help us test new features\n🎥 - Receive social media notifications",
            0x00FF_F11A,
            &config::REACTION_ROLES[..6],
            Some("https://www.6b6t.org/media/language-and-roles.gif"),
        ),
        (
            "🎮 - Get notifications about Hytale.",
            0x0082_C0EF,
            &config::REACTION_ROLES[12..],
            None,
        ),
    ];
    for (description, colour, roles, image) in menus {
        let mut embed = serenity::CreateEmbed::new()
            .author(
                serenity::CreateEmbedAuthor::new("6b6t.org")
                    .icon_url("https://www.6b6t.org/logo.png"),
            )
            .description(description)
            .colour(colour);
        if let Some(image) = image {
            embed = embed.image(image);
        }
        let message = config::REACTION_ROLE_MENU_ID
            .send_message(ctx, serenity::CreateMessage::new().embed(embed))
            .await?;
        for (emoji, _) in roles {
            message
                .react(ctx, serenity::ReactionType::Unicode((*emoji).to_owned()))
                .await?;
        }
    }
    Ok(())
}
