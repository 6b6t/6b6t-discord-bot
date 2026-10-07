use super::{
    BannerService,
    model::{Contest, Schedule},
};
use crate::{
    config,
    state::{Context, Error},
};
use anyhow::{Context as _, Result, bail};
use chrono::{Datelike as _, NaiveDate, Utc};
use poise::serenity_prelude as serenity;
use sqlx::Row as _;

async fn staff(ctx: Context<'_>) -> Result<bool, Error> {
    if ctx.guild_id() != Some(config::GUILD_ID) {
        return Ok(false);
    }
    let member = config::GUILD_ID
        .member(ctx.serenity_context(), ctx.author().id)
        .await?;
    if member
        .roles
        .iter()
        .any(|r| [config::COMMAND_ADMIN_ROLE_ID, config::DEVELOPER_ROLE_ID].contains(r))
    {
        return Ok(true);
    }
    let permissions = ctx
        .author_member()
        .await
        .and_then(|m| m.permissions)
        .unwrap_or_default();
    Ok(permissions.administrator())
}
fn service(ctx: Context<'_>) -> Result<&BannerService> {
    ctx.data()
        .banner_contest
        .as_ref()
        .context("Banner contests require MySQL.")
}
async fn reply(ctx: Context<'_>, text: String) -> Result<()> {
    ctx.send(
        poise::CreateReply::default()
            .content(text)
            .ephemeral(true)
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;
    Ok(())
}

/// Manage upcoming banner themes.
#[poise::command(
    slash_command,
    guild_only,
    check = "staff",
    subcommands("theme_set", "theme_list")
)]
pub async fn bannerthemes(ctx: Context<'_>) -> Result<(), Error> {
    reply(ctx, "Use /bannerthemes set or /bannerthemes list.".into()).await
}
/// Set a theme, or omit it to clear. Discord groups require the 'set' subcommand.
#[poise::command(slash_command, rename = "set")]
async fn theme_set(
    ctx: Context<'_>,
    year: i32,
    month: u32,
    theme: Option<String>,
) -> Result<(), Error> {
    let _ = Schedule::monthly(year, month)?;
    let service = service(ctx)?;
    let _guard = service.lock.lock().await;
    let connection = service
        .acquire()
        .await?
        .context("Contest worker busy; try again.")?;
    let result=async {
        let opened:i64=sqlx::query_scalar("SELECT COUNT(*) FROM banner_contests WHERE year=? AND month=? AND dry_run=FALSE AND (state!='scheduled' OR JSON_EXTRACT(effects,'$.call') IS NOT NULL)").bind(year).bind(month).fetch_one(&service.pool).await?;
        if opened!=0 {bail!("the call has already been posted; its theme cannot change");}
        let theme=theme.as_ref().map(|s|s.trim()).filter(|s|!s.is_empty());
        save_theme(&service.pool, year, month, theme).await?;
        sqlx::query("UPDATE banner_contests SET theme=? WHERE year=? AND month=? AND state='scheduled'").bind(theme).bind(year).bind(month).execute(&service.pool).await?;
        Ok::<_,Error>(())
    }.await;
    BannerService::release(connection).await;
    result?;
    reply(ctx, format!("Theme saved for {year}/{month:02}.")).await
}
/// List upcoming themes.
#[poise::command(slash_command, rename = "list")]
async fn theme_list(ctx: Context<'_>) -> Result<(), Error> {
    let now = Utc::now().with_timezone(&chrono_tz::Europe::Warsaw);
    let rows=sqlx::query("SELECT year,month,theme FROM banner_themes WHERE year>? OR (year=? AND month>=?) ORDER BY year,month LIMIT 24").bind(now.year()).bind(now.year()).bind(now.month()).fetch_all(&service(ctx)?.pool).await?;
    let text = rows
        .iter()
        .map(|r| {
            format!(
                "{}/{:02}: {}",
                r.get::<i32, _>("year"),
                r.get::<u32, _>("month"),
                r.get::<String, _>("theme")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    reply(
        ctx,
        if text.is_empty() {
            "No upcoming themes.".into()
        } else {
            text
        },
    )
    .await
}
/// Manage banner contests.
#[poise::command(
    slash_command,
    guild_only,
    check = "staff",
    subcommands(
        "contest_status",
        "contest_skip",
        "contest_start",
        "contest_test",
        "contest_selftest"
    )
)]
pub async fn bannercontest(ctx: Context<'_>) -> Result<(), Error> {
    reply(
        ctx,
        "Use /bannercontest status, skip, start, test or selftest.".into(),
    )
    .await
}
/// Show contest phase, entries, and the next scheduled step.
#[poise::command(slash_command, rename = "status")]
async fn contest_status(ctx: Context<'_>) -> Result<(), Error> {
    let service = service(ctx)?;
    let contests: Vec<Contest> =
        sqlx::query_as("SELECT * FROM banner_contests WHERE contest_key<>'gateway-stop' AND contest_key NOT LIKE 'selftest-%' ORDER BY id DESC LIMIT 8")
            .fetch_all(&service.pool)
            .await?;
    let mut lines = Vec::new();
    for c in contests {
        let entries = service.entries(c.id).await?;
        let count = |s| entries.iter().filter(|e| e.status == s).count();
        let next = match c.state.as_str() {
            "scheduled" => format!("call <t:{}:F>", c.call_at),
            "open" => format!("close <t:{}:F>", c.close_at),
            "review" => format!("voting <t:{}:F>", c.voting_at),
            "voting" => format!("winner <t:{}:F>", c.end_at),
            _ => "none".into(),
        };
        lines.push(format!("#{} {}/{:02}{}: {}; {} entries ({} approved, {} denied, {} pending, {} expired); next: {next}",c.id,c.year,c.month,if c.dry_run{" TEST"}else{""},c.state,entries.len(),count("approved"),count("denied"),count("pending"),count("expired")));
    }
    reply(
        ctx,
        if lines.is_empty() {
            "No contests scheduled.".into()
        } else {
            lines.join("\n")
        },
    )
    .await
}
/// Skip a monthly contest. Its banner will stay unchanged.
#[poise::command(slash_command, rename = "skip")]
async fn contest_skip(ctx: Context<'_>, year: i32, month: u32) -> Result<(), Error> {
    let schedule = Schedule::monthly(year, month)?;
    let service = service(ctx)?;
    ctx.defer_ephemeral().await?;
    let _guard = service.lock.lock().await;
    let connection = service
        .acquire()
        .await?
        .context("Contest worker busy; try again.")?;
    let result = async {
        let key = format!("{year:04}-{month:02}");
        let existing: Option<u64> =
            sqlx::query_scalar("SELECT id FROM banner_contests WHERE contest_key=?")
                .bind(&key)
                .fetch_optional(&service.pool)
                .await?;
        let id = if let Some(id) = existing {
            id
        } else {
            service.insert_contest(year, month, schedule, false).await?
        };
        let contest = service.contest(id).await?;
        if contest.state == "complete" {
            bail!("contest already completed");
        }
        service.set_state(id, "skipped").await?;
        service.disable_apply(&contest).await?;
        for entry in service.entries(id).await? {
            service.update_review(&entry, true).await?;
        }
        Ok::<_, Error>(())
    }
    .await;
    BannerService::release(connection).await;
    result?;
    reply(ctx, format!("Skipped {year}/{month:02}.")).await
}
/// Start a one-off contest now. End date must be YYYY-MM-DD (Warsaw 10:00).
#[poise::command(slash_command, rename = "start")]
async fn contest_start(
    ctx: Context<'_>,
    year: i32,
    month: u32,
    #[rename = "end-date"] end_date: String,
) -> Result<(), Error> {
    let _ = Schedule::monthly(year, month)?;
    let mut schedule = Schedule::ending(NaiveDate::parse_from_str(&end_date, "%Y-%m-%d")?)?;
    let now = Utc::now().timestamp();
    if schedule.close <= now {
        bail!(
            "end date must leave time for submissions: close is four days before at 22:00 Warsaw"
        );
    }
    schedule.call = now;
    start(ctx, year, month, schedule, false).await
}
/// Run the real form, reviews and voting in banner-reviews, with short phases.
#[poise::command(slash_command, rename = "test")]
async fn contest_test(
    ctx: Context<'_>,
    #[description = "Minutes per phase (1-30, default 1)"] minutes: Option<u32>,
) -> Result<(), Error> {
    let now = Utc::now().with_timezone(&chrono_tz::Europe::Warsaw);
    start(
        ctx,
        now.year(),
        now.month(),
        Schedule::test_with(now.timestamp(), minutes.unwrap_or(1)),
        true,
    )
    .await
}
async fn start(
    ctx: Context<'_>,
    year: i32,
    month: u32,
    schedule: Schedule,
    dry: bool,
) -> Result<()> {
    ctx.defer_ephemeral().await?;
    let service = service(ctx)?;
    let _guard = service.lock.lock().await;
    let connection = service
        .acquire()
        .await?
        .context("Contest worker busy; try again.")?;
    let result=async {
        if dry {
            let tests:i64=sqlx::query_scalar("SELECT COUNT(*) FROM banner_contests WHERE dry_run=TRUE AND state NOT IN ('complete','skipped','failed','paused')").fetch_one(&service.pool).await?;
            if tests>0 {bail!("a test is already running");}
        }
        let id=service.insert_contest(year,month,schedule,dry).await?;
        let contest=service.contest(id).await?;
        if service.call(&contest).await.is_err() {service.fail(id,"Call failed. Inspect the call effect and reconcile the announcement message by hand before resuming.").await?;bail!("call failed; see banner-reviews");}
        Ok::<_,Error>(id)
    }.await;
    BannerService::release(connection).await;
    let id = result?;
    reply(
        ctx,
        format!(
            "Contest #{id} started{}.",
            if dry {
                " in banner-reviews (test mode)"
            } else {
                ""
            }
        ),
    )
    .await
}

pub(super) async fn save_theme(
    pool: &sqlx::MySqlPool,
    year: i32,
    month: u32,
    theme: Option<&str>,
) -> Result<()> {
    if let Some(theme) = theme
        && (theme.chars().count() > 200 || theme.contains(['\n', '\r']))
    {
        bail!("theme must be one line, at most 200 characters");
    }
    sqlx::query("INSERT INTO banner_themes(year,month,theme) VALUES(?,?,?) ON DUPLICATE KEY UPDATE theme=VALUES(theme)").bind(year).bind(month).bind(theme.unwrap_or("")).execute(pool).await?;
    Ok(())
}

/// Exercise current guild images, a one-minute prize and a real staff notification.
#[poise::command(slash_command, rename = "selftest")]
async fn contest_selftest(
    ctx: Context<'_>,
    username: Option<String>,
    group: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let username = if let Some(name) = username {
        name
    } else {
        let databases = ctx
            .data()
            .databases
            .as_ref()
            .context("Database unavailable")?;
        let mapping = databases
            .mapping_for_discord(&ctx.author().id.to_string())
            .await?
            .context("Supply username or link your Minecraft account")?;
        databases
            .player_info(&mapping.uuid)
            .await?
            .context("Linked player not found")?
            .name
    };
    let result = service(ctx)?
        .selftest(
            &ctx.data().server,
            ctx.author().id.get(),
            &username,
            group.as_deref().unwrap_or("primeultra"),
        )
        .await;
    reply(
        ctx,
        if result.is_ok() {
            "Selftest completed; see #banner-reviews.".into()
        } else {
            format!(
                "Selftest failed: {}. See #banner-reviews.",
                result.err().context("missing selftest failure")?
            )
        },
    )
    .await
}
