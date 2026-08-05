use std::fmt::Write as _;

use anyhow::Context as _;
use poise::{CreateReply, serenity_prelude as serenity};

use crate::{
    command_moderation, config,
    server::format_duration,
    state::{AppState, Context, Error},
};

pub fn all() -> Vec<poise::Command<AppState, Error>> {
    vec![
        ip(),
        playercount(),
        version(),
        shop(),
        boost(),
        hytaleplayers(),
        getuser(),
        banreason(),
        command_moderation::discordbannerset(),
        command_moderation::terminatorban(),
        command_moderation::miniterminator(),
        command_moderation::mediachannelsfreq(),
    ]
}

/// See 6b6t's IP.
#[poise::command(slash_command, user_cooldown = 60)]
async fn ip(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("Join 6b6t using the IP `play.6b6t.org` on Java Edition and `bedrock.6b6t.org` with the port 19132 on Bedrock Edition.").await?;
    Ok(())
}

/// See 6b6t's current player count and uptime.
#[poise::command(slash_command, user_cooldown = 60)]
async fn playercount(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let data = match ctx.data().server.server_data().await {
        Ok(data) => data,
        Err(error) => {
            tracing::error!(%error, "failed to fetch server data");
            ctx.send(
                CreateReply::default()
                    .ephemeral(true)
                    .content("Failed to get server data"),
            )
            .await?;
            return Ok(());
        }
    };
    let uptime = data
        .server_start_unix
        .map(|started| chrono::Utc::now().timestamp() - started)
        .or_else(|| {
            data.current_uptime_hours.and_then(|hours| {
                i64::try_from(std::time::Duration::from_secs_f64(hours * 3_600.0).as_secs()).ok()
            })
        });
    let mut message = format!(
        "There are currently {} players online on 6b6t.",
        data.player_count
    );
    if let Some(uptime) = uptime {
        let _ = write!(
            message,
            " The server has been up for {}.",
            format_duration(uptime)
        );
    }
    ctx.say(message).await?;
    Ok(())
}

/// See 6b6t's version.
#[poise::command(slash_command, user_cooldown = 60)]
async fn version(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let version = ctx
        .data()
        .server
        .server_data()
        .await
        .ok()
        .and_then(|data| data.version);
    let Some(version) = version else {
        ctx.send(
            CreateReply::default()
                .ephemeral(true)
                .content("Failed to get server version"),
        )
        .await?;
        return Ok(());
    };
    ctx.say(format!("The current version of 6b6t is {version}. Connect to 6b6t using the IP `play.6b6t.org` on Java Edition and `bedrock.6b6t.org` with the port 19132 on Bedrock Edition.")).await?;
    Ok(())
}

/// See 6b6t's shop.
#[poise::command(slash_command, user_cooldown = 60)]
async fn shop(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("You can support 6b6t financially and get benefits like more homes, lower /tpa and /home cooldowns, access to commands like /hat, /balloons, /chatcolor and much more at the [6b6t Shop](<https://www.6b6t.org/shop?utm_source=discord&utm_medium=discord_command&utm_campaign=evergreen_shop&utm_content=shop&lang=en>).").await?;
    Ok(())
}

/// See the perks for boosting the 6b6t Discord.
#[poise::command(slash_command, user_cooldown = 60)]
async fn boost(ctx: Context<'_>) -> Result<(), Error> {
    ctx.send(CreateReply::default().content("Boosting the 6b6t Discord gives you the <@&933418896692768820> role, nickname changing permissions, embed, media and emoji permissions in general and free access to priority support in the Discord channel #📜premium-tickets.").allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Check the current 6b6t Hytale player count.
#[poise::command(slash_command)]
async fn hytaleplayers(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let data = match ctx.data().server.hytale_data().await {
        Ok(data) => data,
        Err(error) => {
            tracing::error!(%error, "failed to fetch Hytale player data");
            ctx.send(
                CreateReply::default()
                    .ephemeral(true)
                    .content("Failed to get Hytale player count."),
            )
            .await?;
            return Ok(());
        }
    };
    let mut message = format!(
        "There are currently {}/{} players online on 6b6t Hytale.",
        data.player_count, data.max_players
    );
    if let Some(metrics) = data.metrics {
        let mut parts = Vec::new();
        if let Some(tps) = metrics.tps {
            parts.push(format!("**TPS**: {tps:.1}"));
        }
        if let Some(entities) = metrics.entities {
            parts.push(format!("**Entities**: {entities}"));
        }
        if let Some(chunks) = metrics.chunks {
            parts.push(format!("**Chunks**: {chunks}"));
        }
        if !parts.is_empty() {
            let _ = write!(message, "\n{}", parts.join(" | "));
        }
    }
    if !data.players.is_empty() {
        let _ = write!(
            message,
            "\n**Online players**: {}",
            data.players
                .iter()
                .map(|player| player.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    ctx.say(message).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "ADMINISTRATOR"
)]
/// Look up the Minecraft account linked to a Discord user.
async fn getuser(
    ctx: Context<'_>,
    #[description = "Discord user to look up"] id: serenity::User,
) -> Result<(), Error> {
    if !command_admin(&ctx).await {
        ctx.send(
            CreateReply::default()
                .ephemeral(true)
                .content("You do not have permission to use this command."),
        )
        .await?;
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    match ctx.data().server.player_for_discord(id.id.get()).await {
        Ok(Some((name, info))) => {
            ctx.say(format!(
                "**{}** is linked to **{name}**.\nTop Rank: **{}**\nFirst Join Year: **{}**",
                id.tag(),
                info.top_rank,
                info.first_join_year
            ))
            .await?
        }
        Ok(None) => {
            ctx.say(format!("No Minecraft account linked for {}.", id.tag()))
                .await?
        }
        Err(error) => {
            tracing::error!(%error, "failed to look up linked user");
            ctx.say("An error occurred while fetching user info.")
                .await?
        }
    };
    Ok(())
}

/// Show the ban reason for a user.
#[poise::command(slash_command, guild_only)]
async fn banreason(
    ctx: Context<'_>,
    #[description = "User to check"] user: serenity::User,
) -> Result<(), Error> {
    let Some(member) = ctx.author_member().await else {
        return Ok(());
    };
    if !config::BAN_REASON_ROLE_IDS
        .iter()
        .any(|role| member.roles.contains(role))
    {
        ctx.send(
            CreateReply::default()
                .ephemeral(true)
                .content("You do not have permission to use this command."),
        )
        .await?;
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().context("banreason used outside a guild")?;
    let Some(ban) = guild_id.get_ban(ctx.http(), user.id).await? else {
        ctx.say(format!(
            "{} is not currently banned from this server.",
            user.tag()
        ))
        .await?;
        return Ok(());
    };
    let audit = guild_id
        .audit_logs(
            ctx.http(),
            Some(serenity::audit_log::Action::Member(
                serenity::audit_log::MemberAction::BanAdd,
            )),
            None,
            None,
            Some(10),
        )
        .await
        .ok();
    let entry = audit.as_ref().and_then(|logs| {
        logs.entries.iter().find(|entry| {
            entry
                .target_id
                .is_some_and(|target| target.get() == user.id.get())
        })
    });
    let reason = ban
        .reason
        .or_else(|| entry.and_then(|entry| entry.reason.clone()))
        .unwrap_or_else(|| "No reason provided.".into());
    let banned_by = entry
        .and_then(|entry| audit.as_ref()?.users.get(&entry.user_id))
        .map_or_else(|| "Unknown (not found)".into(), serenity::User::tag);
    ctx.say(format!(
        "**Ban information for {}:**\n• Reason: {reason}\n• Banned by: {banned_by}",
        user.tag()
    ))
    .await?;
    Ok(())
}

async fn command_admin(ctx: &Context<'_>) -> bool {
    ctx.author_member()
        .await
        .is_some_and(|member| member.roles.contains(&config::COMMAND_ADMIN_ROLE_ID))
}
