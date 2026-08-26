use std::fmt::Write as _;

use anyhow::Context as _;
use poise::{CreateReply, serenity_prelude as serenity};

use crate::{
    command_moderation, config,
    database::Databases,
    moderation,
    server::format_duration,
    state::{AppState, Context, Error},
};

const ANARCHY_MOD_MESSAGE: &str = "Mojang banned 6b6t. Read what happened in our [blog post](<https://blog.6b6t.org/minecraft-banned-6b6t/>). 6b6t remains true anarchy without rules or punishments. To join on Java Edition, you have to download [AnarchyMod](<https://6b6t.org/mod>).";

pub fn all() -> Vec<poise::Command<AppState, Error>> {
    vec![
        ip(),
        anarchymod(),
        playercount(),
        version(),
        shop(),
        boost(),
        hytaleplayers(),
        getuser(),
        banreason(),
        assignyoutuber(),
        removeyoutuber(),
        command_moderation::discordbannerset(),
        command_moderation::terminatorban(),
        command_moderation::terminatorunban(),
        command_moderation::miniterminator(),
        command_moderation::mediachannelsfreq(),
        command_moderation::purge(),
        command_moderation::reapplylanguages(),
    ]
}

/// See 6b6t's IP.
#[poise::command(slash_command, user_cooldown = 60)]
async fn ip(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("Join 6b6t using the IP `bedrock.6b6t.org` with the port 19132 on Bedrock Edition. To play 6b6t on Java Edition, download [AnarchyMod](<https://6b6t.org/mod>) and join using the IP `play.6b6t.org`.").await?;
    Ok(())
}

/// Learn how to join 6b6t on Java Edition with `AnarchyMod`.
#[poise::command(slash_command, user_cooldown = 60)]
async fn anarchymod(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say(ANARCHY_MOD_MESSAGE).await?;
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
            finish_deferred(ctx, "Failed to get server data", true).await?;
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
    message.push_str(" To play, download [AnarchyMod](<https://6b6t.org/mod>).");
    finish_deferred(ctx, message, false).await?;
    Ok(())
}

/// See 6b6t's version.
#[poise::command(slash_command, user_cooldown = 60)]
async fn version(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("The current version of 6b6t is 26.2. To play on Java Edition, download [AnarchyMod](<https://6b6t.org/mod>).").await?;
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
    send_suppressed(ctx, "Boosting the 6b6t Discord gives you the <@&933418896692768820> role, nickname changing permissions, embed, media and emoji permissions in general and free access to priority support in the Discord channel #📜premium-tickets.").await?;
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
            finish_deferred(ctx, "Failed to get Hytale player count.", true).await?;
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
    finish_deferred(ctx, message, false).await?;
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
    let mut before = None;
    let mut audit_match = None;
    // Discord cannot filter audit logs by target. Walk several full pages so
    // bans older than the latest ten moderation actions still resolve.
    for _ in 0..10 {
        let logs = match guild_id
            .audit_logs(
                ctx.http(),
                Some(serenity::audit_log::Action::Member(
                    serenity::audit_log::MemberAction::BanAdd,
                )),
                None,
                before,
                Some(100),
            )
            .await
        {
            Ok(logs) => logs,
            Err(error) => {
                tracing::warn!(%error, target_id = %user.id, "failed to fetch ban audit logs");
                break;
            }
        };
        audit_match = logs.entries.iter().find_map(|entry| {
            entry
                .target_id
                .is_some_and(|target| target.get() == user.id.get())
                .then(|| (entry.user_id, entry.reason.clone()))
        });
        if audit_match.is_some() || logs.entries.len() < 100 {
            break;
        }
        before = logs.entries.last().map(|entry| entry.id);
    }
    let reason = ban
        .reason
        .or_else(|| audit_match.as_ref().and_then(|(_, reason)| reason.clone()))
        .unwrap_or_else(|| "No reason provided.".into());
    let banned_by = if let Some((moderator_id, _)) = audit_match {
        match moderator_id.to_user(ctx.http()).await {
            Ok(moderator) => format!("{} (<@{moderator_id}>)", moderator.tag()),
            Err(error) => {
                tracing::warn!(%error, %moderator_id, "failed to resolve banning moderator");
                format!("<@{moderator_id}>")
            }
        }
    } else {
        "Unknown (audit entry not found)".into()
    };
    ctx.say(format!(
        "**Ban information for {}:**\n• Reason: {reason}\n• Banned by: {banned_by}",
        user.tag()
    ))
    .await?;
    Ok(())
}

/// Assign the `youtuber` rank and `YouTuber` Discord role by player name or Discord user.
#[poise::command(slash_command, guild_only)]
async fn assignyoutuber(
    ctx: Context<'_>,
    #[description = "Minecraft player name or Discord user tag"] player_name: String,
) -> Result<(), Error> {
    youtuber_role(ctx, player_name, true).await
}

/// Remove the `youtuber` rank and `YouTuber` Discord role by player name or Discord user.
#[poise::command(slash_command, guild_only)]
async fn removeyoutuber(
    ctx: Context<'_>,
    #[description = "Minecraft player name or Discord user tag"] player_name: String,
) -> Result<(), Error> {
    youtuber_role(ctx, player_name, false).await
}

async fn youtuber_role(ctx: Context<'_>, player_name: String, add: bool) -> Result<(), Error> {
    let member = ctx
        .author_member()
        .await
        .context("missing command member")?;
    if !moderation::is_administrator(&member) && !member.roles.contains(&config::MARKETER_ROLE_ID) {
        deny(
            ctx,
            "You do not have permission to use this command. Required role: Marketer.",
        )
        .await?;
        return Ok(());
    }
    let databases = ctx
        .data()
        .databases
        .as_ref()
        .context("account linking is not available")?;
    let uuid = match resolve_player_uuid(databases, &player_name).await? {
        PlayerUuidResolution::Found(uuid) => uuid,
        PlayerUuidResolution::NotFound => {
            let message = if parse_discord_mention(&player_name).is_some() {
                "No Minecraft account is linked to that Discord user."
            } else {
                "No player found with the name `{player_name}`."
            };
            ctx.say(message).await?;
            return Ok(());
        }
        PlayerUuidResolution::Ambiguous => {
            ctx.say(format!(
                "Multiple Minecraft accounts use the name `{player_name}` and no unique linked account could be selected. Run the command again using the linked Discord user mention."
            ))
            .await?;
            return Ok(());
        }
    };
    let guild_id = ctx.guild_id().context("missing guild")?;
    let action = if add { "add" } else { "remove" };
    ctx.defer().await?;
    ctx.data()
        .server
        .run_lp_command(&uuid, action, config::YOUTUBE_RANK_NAME)
        .await?;

    let role_id = config::YOUTUBER_ROLE_ID;
    let mut discord_outcome = "No linked Discord account to update.".to_owned();
    if let Some(mapping) = databases.mapping_for_uuid(&uuid).await?
        && let Ok(discord_id) = mapping.discord_id.parse::<u64>()
    {
        let discord_id = serenity::UserId::new(discord_id);
        if let Ok(target) = guild_id.member(ctx.http(), discord_id).await {
            let has_role = target.roles.contains(&role_id);
            if add == has_role {
                discord_outcome = format!(
                    "<@{discord_id}> {} had the YouTuber Discord role.",
                    if add { "already" } else { "did not" }
                );
            } else {
                let result = if add {
                    target.add_role(ctx, role_id).await
                } else {
                    target.remove_role(ctx, role_id).await
                };
                match result {
                    Ok(()) => {
                        discord_outcome = format!(
                            "YouTuber Discord role {} on <@{discord_id}>.",
                            if add { "assigned" } else { "removed" }
                        );
                    }
                    Err(error) => {
                        tracing::error!(%error, %discord_id, "failed to update YouTuber Discord role");
                        discord_outcome = format!(
                            "Could not update the YouTuber Discord role on <@{discord_id}>."
                        );
                    }
                }
            }
        } else {
            discord_outcome = "The linked Discord account is not in this server.".into();
        }
    }
    ctx.say(format!(
        "The in-game YouTube rank was {} for player `{player_name}` (`{uuid}`). {}",
        if add { "assigned" } else { "removed" },
        discord_outcome
    ))
    .await?;
    Ok(())
}

/// Resolve a Minecraft player's UUID from either a player name in the stats
/// database or a `<@discord_id>` mention through the link database.
#[derive(Debug, PartialEq, Eq)]
enum PlayerUuidResolution {
    Found(String),
    NotFound,
    Ambiguous,
}

async fn resolve_player_uuid(
    databases: &Databases,
    input: &str,
) -> Result<PlayerUuidResolution, anyhow::Error> {
    if let Some(discord_id) = parse_discord_mention(input) {
        return Ok(databases
            .mapping_for_discord(&discord_id.to_string())
            .await?
            .map_or(PlayerUuidResolution::NotFound, |mapping| {
                PlayerUuidResolution::Found(mapping.uuid)
            }));
    }
    let candidates = unique_uuids(databases.uuids_for_player_name(input).await?);
    match candidates.as_slice() {
        [] => Ok(PlayerUuidResolution::NotFound),
        [uuid] => Ok(PlayerUuidResolution::Found(uuid.clone())),
        _ => {
            let mut linked = Vec::new();
            for uuid in &candidates {
                if let Some(mapping) = databases.mapping_for_uuid(uuid).await? {
                    linked.push(mapping.uuid);
                }
            }
            Ok(select_linked_uuid(linked))
        }
    }
}

fn unique_uuids(uuids: Vec<String>) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for uuid in uuids {
        if !unique
            .iter()
            .any(|candidate| normalize_uuid(candidate) == normalize_uuid(&uuid))
        {
            unique.push(uuid);
        }
    }
    unique
}

fn select_linked_uuid(linked: Vec<String>) -> PlayerUuidResolution {
    match unique_uuids(linked).as_slice() {
        [uuid] => PlayerUuidResolution::Found(uuid.clone()),
        _ => PlayerUuidResolution::Ambiguous,
    }
}

fn normalize_uuid(uuid: &str) -> String {
    uuid.chars()
        .filter(|character| *character != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Parse `<@123...>` or the legacy `<@!123...>` user mention into a snowflake.
fn parse_discord_mention(value: &str) -> Option<u64> {
    let inner = value.strip_prefix("<@")?.strip_suffix('>')?;
    inner
        .strip_prefix('!')
        .or(Some(inner))
        .and_then(|id| id.parse::<u64>().ok())
}

async fn command_admin(ctx: &Context<'_>) -> bool {
    ctx.author_member()
        .await
        .is_some_and(|member| member.roles.contains(&config::COMMAND_ADMIN_ROLE_ID))
}

async fn finish_deferred(
    ctx: Context<'_>,
    content: impl Into<String>,
    ephemeral: bool,
) -> Result<(), Error> {
    let content = content.into();
    let poise::Context::Application(application) = ctx else {
        ctx.say(content).await?;
        return Ok(());
    };
    if ephemeral {
        application
            .interaction
            .create_followup(
                application.serenity_context,
                serenity::CreateInteractionResponseFollowup::new()
                    .content(content)
                    .ephemeral(true)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await?;
        if let Err(error) = application
            .interaction
            .delete_response(application.serenity_context)
            .await
        {
            tracing::warn!(%error, "failed to remove public deferred response");
        }
    } else {
        application
            .interaction
            .edit_response(
                application.serenity_context,
                serenity::EditInteractionResponse::new()
                    .content(content)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await?;
    }
    Ok(())
}

async fn deny(ctx: Context<'_>, message: &str) -> Result<(), Error> {
    ctx.send(CreateReply::default().ephemeral(true).content(message))
        .await?;
    Ok(())
}

async fn send_suppressed(ctx: Context<'_>, content: &str) -> Result<(), Error> {
    let poise::Context::Application(application) = ctx else {
        ctx.say(content).await?;
        return Ok(());
    };
    application
        .interaction
        .create_response(
            application.serenity_context,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(content)
                    .allowed_mentions(serenity::CreateAllowedMentions::new())
                    .flags(serenity::InteractionResponseFlags::SUPPRESS_NOTIFICATIONS),
            ),
        )
        .await?;
    application
        .has_sent_initial_response
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PlayerUuidResolution, parse_discord_mention, select_linked_uuid, unique_uuids};

    #[test]
    fn user_mentions_parse_to_snowflakes() {
        assert_eq!(
            parse_discord_mention("<@681552494064697350>"),
            Some(681_552_494_064_697_350)
        );
        assert_eq!(
            parse_discord_mention("<@!681552494064697350>"),
            Some(681_552_494_064_697_350)
        );
    }

    #[test]
    fn channel_and_role_mentions_are_not_users() {
        assert!(parse_discord_mention("<#123456789>").is_none());
        assert!(parse_discord_mention("<@&123456789>").is_none());
    }

    #[test]
    fn plain_player_names_are_not_mentions() {
        assert!(parse_discord_mention("ExampleName").is_none());
        assert!(parse_discord_mention("<@notanumber>").is_none());
    }

    #[test]
    fn duplicate_player_names_select_the_only_linked_uuid() {
        assert_eq!(
            select_linked_uuid(vec!["a8234a1d-244d-48df-afd2-4bce607bcd8f".into()]),
            PlayerUuidResolution::Found("a8234a1d-244d-48df-afd2-4bce607bcd8f".into())
        );
    }

    #[test]
    fn duplicate_player_names_fail_closed_without_one_link() {
        assert_eq!(
            select_linked_uuid(Vec::new()),
            PlayerUuidResolution::Ambiguous
        );
        assert_eq!(
            select_linked_uuid(vec!["first".into(), "second".into()]),
            PlayerUuidResolution::Ambiguous
        );
    }

    #[test]
    fn uuid_deduplication_ignores_case_and_hyphens() {
        assert_eq!(
            unique_uuids(vec![
                "A8234A1D-244D-48DF-AFD2-4BCE607BCD8F".into(),
                "a8234a1d244d48dfafd24bce607bcd8f".into(),
            ]),
            vec!["A8234A1D-244D-48DF-AFD2-4BCE607BCD8F"]
        );
    }
}
