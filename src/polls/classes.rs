//! The player classes, evaluated with SQL against the `player_stats` database.
//!
//! The queries follow section 4 of the `PlayerActivity` data contract
//! (`CONTRACT.md`, owned by the plugin PR). Every query takes the poll's
//! cut-off. Event rows (fights) must lie wholly before `cutoff_ms`; day rows
//! must be on a UTC day strictly before the cut-off day, so the cut-off day
//! itself never counts. A class never looks at anything newer, which is what
//! freezes eligibility at poll start. The cut-off is always computed in the bot
//! and bound as a parameter, never taken from the database session clock.

use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{MySqlPool, Row as _};

use super::{
    config::{
        ActivityConfig, BuilderConfig, CrystalConfig, PollConfig, TICKS_PER_MINUTE, VeteranConfig,
    },
    expr::{Class, ClassCall},
    identity::Identity,
};
use crate::database::normalize_uuid;

const DAY_MS: i64 = 86_400_000;

/// A class with its thresholds fully resolved (config defaults plus the
/// poll's own overrides). Stored in `polls.rule_json` for the audit trail and
/// never rendered to players.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum Rule {
    Veteran(VeteranConfig),
    OverallActive(ActivityConfig),
    Active(ActivityConfig),
    VeryActive(ActivityConfig),
    CrystalPvper(CrystalConfig),
    Builder(BuilderConfig),
}

impl Rule {
    /// The class with the poll's own overrides laid over its configured
    /// settings. A class without settings in `POLLS_CONFIG` cannot be used.
    pub fn resolve(call: &ClassCall, config: &PollConfig) -> Result<Self, String> {
        let missing = || {
            format!(
                "The class `{}` is not configured on this bot, so polls cannot use it yet.",
                call.class.name()
            )
        };
        let activity = |base: Option<ActivityConfig>| -> Result<ActivityConfig, String> {
            let mut resolved = base.ok_or_else(missing)?;
            if let Some(value) = call.get("days") {
                resolved.min_days = value;
            }
            if let Some(value) = call.get("window") {
                resolved.window_days = value;
            }
            if let Some(value) = call.get("minutes") {
                resolved.min_minutes = value;
            }
            Ok(resolved)
        };
        Ok(match call.class {
            Class::Veteran => {
                let mut resolved = config.veteran.ok_or_else(missing)?;
                if let Some(value) = call.get("days") {
                    resolved.days = value;
                }
                Self::Veteran(resolved)
            }
            Class::OverallActive => Self::OverallActive(activity(config.overall_active)?),
            Class::Active => Self::Active(activity(config.active)?),
            Class::VeryActive => Self::VeryActive(activity(config.very_active)?),
            Class::CrystalPvper => {
                let mut resolved = config.crystal_pvper.ok_or_else(missing)?;
                if let Some(value) = call.get("fights") {
                    resolved.min_fights = value;
                }
                if let Some(value) = call.get("opponents") {
                    resolved.min_opponents = value;
                }
                if let Some(value) = call.get("damage") {
                    resolved.min_damage = value;
                }
                if let Some(value) = call.get("window") {
                    resolved.window_days = value;
                }
                Self::CrystalPvper(resolved)
            }
            Class::Builder => {
                let mut resolved = config.builder.ok_or_else(missing)?;
                if let Some(value) = call.get("days") {
                    resolved.build_days = value;
                }
                if let Some(value) = call.get("window") {
                    resolved.window_days = value;
                }
                if let Some(value) = call.get("placed") {
                    resolved.placed = value;
                }
                if let Some(value) = call.get("materials") {
                    resolved.materials = value;
                }
                Self::Builder(resolved)
            }
        })
    }

    /// The look-back window in days, for classes that have one.
    pub fn window_days(&self) -> Option<i64> {
        match self {
            Self::Veteran(_) => None,
            Self::OverallActive(config) | Self::Active(config) | Self::VeryActive(config) => {
                Some(config.window_days)
            }
            Self::CrystalPvper(config) => Some(config.window_days),
            Self::Builder(config) => Some(config.window_days),
        }
    }

    /// The `activity_meta` recording-start key this class depends on, if the
    /// class reads plugin data at all.
    pub fn recording_key(&self) -> Option<&'static str> {
        match self {
            Self::CrystalPvper(_) => Some("recording_since.crystal"),
            Self::Builder(_) => Some("recording_since.build"),
            _ => None,
        }
    }
}

/// The UTC calendar day of the cut-off. Day rows strictly before it count.
pub fn cutoff_date(cutoff_ms: i64) -> NaiveDate {
    DateTime::<Utc>::from_timestamp_millis(cutoff_ms)
        .unwrap_or_default()
        .date_naive()
}

/// Everything a class needs besides its own thresholds.
pub struct EvalContext<'a> {
    pub stats: &'a MySqlPool,
    pub cutoff_ms: i64,
    pub identity: &'a Identity,
    pub bots: &'a HashSet<String>,
}

pub async fn evaluate(rule: &Rule, ctx: &EvalContext<'_>) -> Result<HashSet<String>> {
    let date = cutoff_date(ctx.cutoff_ms);
    match rule {
        Rule::Veteran(config) => veterans(ctx.stats, ctx.cutoff_ms, config.days).await,
        Rule::OverallActive(config) | Rule::Active(config) | Rule::VeryActive(config) => {
            active_days(ctx.stats, date, config).await
        }
        Rule::CrystalPvper(config) => crystal_pvpers(ctx, config).await,
        Rule::Builder(config) => builders(ctx.stats, date, config).await,
    }
}

fn normalized(values: &[String]) -> HashSet<String> {
    values.iter().map(|uuid| normalize_uuid(uuid)).collect()
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` from 32 hex digits, so lookups use
/// the primary key. Anything else is returned unchanged.
fn dashed(uuid: &str) -> String {
    if uuid.len() == 32 && uuid.is_ascii() {
        format!(
            "{}-{}-{}-{}-{}",
            &uuid[..8],
            &uuid[8..12],
            &uuid[12..16],
            &uuid[16..20],
            &uuid[20..]
        )
    } else {
        uuid.to_owned()
    }
}

/// Veteran (contract 4.2): first joined more than `days` before the cut-off.
/// A premium uuid also inherits the earliest `first_join` of a cracked
/// (offline, UUID version 3) row with the same name, which covers cracked to
/// premium conversions without letting one premium account inherit another's
/// date.
async fn veterans(pool: &MySqlPool, cutoff_ms: i64, days: i64) -> Result<HashSet<String>> {
    let limit = cutoff_ms.saturating_sub(days.saturating_mul(DAY_MS));
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT pi.uuid FROM player_info pi \
         LEFT JOIN (SELECT LOWER(name) AS lname, MIN(first_join) AS fj FROM player_info WHERE SUBSTRING(uuid, 15, 1) = '3' GROUP BY LOWER(name)) cracked ON cracked.lname = LOWER(pi.name) \
         WHERE LEAST(pi.first_join, COALESCE(cracked.fj, pi.first_join)) < ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("failed to evaluate the veteran class")?;
    Ok(normalized(&rows))
}

/// The three activity tiers (contract 4.3): at least `min_days` UTC days with
/// at least `min_minutes` of play inside the `window_days` whole days before
/// the cut-off day. Play time is in ticks (1,200 a minute). It counts days with
/// play time, not the `joins` stat, which counts worker hops.
async fn active_days(
    pool: &MySqlPool,
    cutoff: NaiveDate,
    config: &ActivityConfig,
) -> Result<HashSet<String>> {
    let first_day = cutoff - Duration::days(config.window_days);
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT uuid FROM (\
            SELECT uuid, day, SUM(value) AS ticks FROM player_stats_per_day \
            WHERE type = 'play_time' AND day >= ? AND day < ? GROUP BY uuid, day HAVING ticks >= ?\
         ) days GROUP BY uuid HAVING COUNT(*) >= ?",
    )
    .bind(first_day)
    .bind(cutoff)
    .bind(config.min_minutes.saturating_mul(TICKS_PER_MINUTE))
    .bind(config.min_days)
    .fetch_all(pool)
    .await
    .context("failed to evaluate an activity class")?;
    Ok(normalized(&rows))
}

/// One crystal fight, as stored (the pair row).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fight {
    pub a: String,
    pub b: String,
    pub started_at: i64,
    pub ended_at: i64,
    /// Both sides' crystal damage together, tenths of HP.
    pub damage: i64,
}

/// What the crystal rule needs to judge opponents.
pub struct FightContext<'a> {
    pub cutoff_ms: i64,
    pub identity: &'a Identity,
    pub bots: &'a HashSet<String>,
    /// First join (ms) per normalized uuid, for the opponent age check.
    pub first_join: &'a HashMap<String, i64>,
}

/// The crystal `PvPer` rule (contract 4.1) over stored fights. A fight counts for
/// a player when it lies wholly inside the window and before the cut-off, is
/// big enough, and:
/// - neither side is bot-marked,
/// - the opponent has a `player_info` row older than the minimum age,
/// - the opponent did not share a (non-crowded) IP address with the player,
/// - the pair has not already fought `pair_day_cap` times that UTC day.
///
/// The player needs `min_fights` such fights against `min_opponents` distinct
/// opponents.
pub fn judge_crystal_fights(
    fights: &[Fight],
    config: &CrystalConfig,
    ctx: &FightContext<'_>,
) -> HashSet<String> {
    let window_start = ctx
        .cutoff_ms
        .saturating_sub(config.window_days.saturating_mul(DAY_MS));
    let mut ordered: Vec<&Fight> = fights
        .iter()
        .filter(|fight| {
            fight.started_at >= window_start
                && fight.ended_at < ctx.cutoff_ms
                && fight.damage >= config.min_damage
        })
        .collect();
    ordered.sort_by_key(|fight| fight.started_at);

    let youngest_allowed = ctx
        .cutoff_ms
        .saturating_sub(config.opponent_min_age_days.saturating_mul(DAY_MS));
    let mut per_pair_day: HashMap<(String, String, i64), i64> = HashMap::new();
    let mut fights_by_player: HashMap<String, i64> = HashMap::new();
    let mut opponents_by_player: HashMap<String, HashSet<String>> = HashMap::new();

    for fight in ordered {
        let a = normalize_uuid(&fight.a);
        let b = normalize_uuid(&fight.b);
        if ctx.bots.contains(&a) || ctx.bots.contains(&b) || ctx.identity.shares_ip(&a, &b) {
            continue;
        }
        for (me, opponent) in [(&a, &b), (&b, &a)] {
            let aged = ctx
                .first_join
                .get(opponent)
                .is_some_and(|first_join| *first_join < youngest_allowed);
            if !aged {
                continue;
            }
            let day = fight.started_at.div_euclid(DAY_MS);
            let seen = per_pair_day
                .entry((me.clone(), opponent.clone(), day))
                .or_insert(0);
            if *seen >= config.pair_day_cap {
                continue;
            }
            *seen += 1;
            *fights_by_player.entry(me.clone()).or_insert(0) += 1;
            opponents_by_player
                .entry(me.clone())
                .or_default()
                .insert(opponent.clone());
        }
    }
    fights_by_player
        .into_iter()
        .filter(|(player, fights)| {
            *fights >= config.min_fights
                && opponents_by_player.get(player).is_some_and(|opponents| {
                    i64::try_from(opponents.len()).unwrap_or(i64::MAX) >= config.min_opponents
                })
        })
        .map(|(player, _)| player)
        .collect()
}

async fn crystal_pvpers(ctx: &EvalContext<'_>, config: &CrystalConfig) -> Result<HashSet<String>> {
    let window_start = ctx
        .cutoff_ms
        .saturating_sub(config.window_days.saturating_mul(DAY_MS));
    let rows = sqlx::query(
        "SELECT a_uuid, b_uuid, started_at, ended_at, CAST(a_dmg AS SIGNED) + CAST(b_dmg AS SIGNED) AS damage \
         FROM activity_crystal_fight WHERE started_at >= ? AND ended_at < ?",
    )
    .bind(window_start)
    .bind(ctx.cutoff_ms)
    .fetch_all(ctx.stats)
    .await
    .context("failed to read crystal fights")?;
    let mut fights = Vec::with_capacity(rows.len());
    let mut involved: HashSet<String> = HashSet::new();
    for row in &rows {
        let fight = Fight {
            a: row.try_get("a_uuid")?,
            b: row.try_get("b_uuid")?,
            started_at: row.try_get("started_at")?,
            ended_at: row.try_get("ended_at")?,
            damage: row.try_get("damage")?,
        };
        involved.insert(normalize_uuid(&fight.a));
        involved.insert(normalize_uuid(&fight.b));
        fights.push(fight);
    }
    let first_join = first_joins(ctx.stats, &involved).await?;
    Ok(judge_crystal_fights(
        &fights,
        config,
        &FightContext {
            cutoff_ms: ctx.cutoff_ms,
            identity: ctx.identity,
            bots: ctx.bots,
            first_join: &first_join,
        },
    ))
}

/// `player_info.first_join` for the given accounts, keyed by normalized uuid.
async fn first_joins(pool: &MySqlPool, uuids: &HashSet<String>) -> Result<HashMap<String, i64>> {
    let mut result = HashMap::new();
    let all: Vec<String> = uuids.iter().map(|uuid| dashed(uuid)).collect();
    for chunk in all.chunks(500) {
        let mut builder = sqlx::QueryBuilder::<sqlx::MySql>::new(
            "SELECT uuid, first_join FROM player_info WHERE uuid IN (",
        );
        let mut separated = builder.separated(", ");
        for uuid in chunk {
            separated.push_bind(uuid.as_str());
        }
        separated.push_unseparated(")");
        for row in builder.build().fetch_all(pool).await? {
            let uuid: String = row.try_get("uuid")?;
            let first_join: i64 = row.try_get("first_join")?;
            result
                .entry(normalize_uuid(&uuid))
                .and_modify(|current: &mut i64| *current = (*current).min(first_join))
                .or_insert(first_join);
        }
    }
    Ok(result)
}

/// Builder (contract 4.4, phase 1: daily deltas of the vanilla counters).
/// Within the `window_days` whole days before the cut-off day: enough blocks
/// placed overall, on enough days, in enough block types, a healthy
/// placed/mined ratio and not an obsidian-farm pattern. Ratios use integer
/// arithmetic (tenths and percent) so the comparison is exact.
async fn builders(
    pool: &MySqlPool,
    cutoff: NaiveDate,
    config: &BuilderConfig,
) -> Result<HashSet<String>> {
    let first_day = cutoff - Duration::days(config.window_days);
    let rows = sqlx::query_scalar::<_, String>(
        "WITH d AS (\
            SELECT uuid, SUM(placed) AS placed, SUM(placed_obsidian) AS obs, SUM(mined) AS mined, SUM(placed >= ?) AS build_days \
            FROM activity_build_day WHERE day >= ? AND day < ? GROUP BY uuid\
         ), m AS (\
            SELECT uuid, COUNT(*) AS materials FROM (\
                SELECT uuid, material FROM activity_build_material WHERE day >= ? AND day < ? \
                GROUP BY uuid, material HAVING SUM(placed) >= ?\
            ) x GROUP BY uuid\
         ) \
         SELECT d.uuid FROM d JOIN m ON m.uuid = d.uuid \
         WHERE d.placed >= ? AND d.build_days >= ? AND m.materials >= ? \
           AND d.placed * 10 >= ? * GREATEST(d.mined, 1) \
           AND d.obs * 100 <= ? * (d.placed + d.obs)",
    )
    .bind(config.min_day_placed)
    .bind(first_day)
    .bind(cutoff)
    .bind(first_day)
    .bind(cutoff)
    .bind(config.min_material_placed)
    .bind(config.placed)
    .bind(config.build_days)
    .bind(config.materials)
    .bind(config.placed_per_mined_tenths)
    .bind(config.max_obsidian_percent)
    .fetch_all(pool)
    .await
    .context("failed to evaluate the builder class")?;
    Ok(normalized(&rows))
}

/// Whether a table exists in the stats database. The plugin creates the
/// `activity_*` tables; until it has run, only veteran and the activity tiers
/// can be evaluated.
pub async fn table_exists(pool: &MySqlPool, table: &str) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

/// Bot-marked accounts, as recorded by the plugin (`LuckPerms` `6b6t-bot` meta
/// and the tempbot group). Read at snapshot time, so late marking still works.
/// An account without a row is "not known as a bot".
pub async fn bot_accounts(pool: &MySqlPool) -> Result<HashSet<String>> {
    let rows = sqlx::query_scalar::<_, String>("SELECT uuid FROM activity_player WHERE is_bot = 1")
        .fetch_all(pool)
        .await
        .context("failed to read bot flags")?;
    Ok(normalized(&rows))
}

/// Distinct `(uuid, ip_hash)` pairs on the UTC days in the `window_days`
/// before the cut-off day. Hashes are keyed by the plugin; plain IPs are never
/// stored anywhere.
pub async fn ip_observations(
    pool: &MySqlPool,
    cutoff_ms: i64,
    window_days: i64,
) -> Result<Vec<(String, Vec<u8>)>> {
    let cutoff = cutoff_date(cutoff_ms);
    let first_day = cutoff - Duration::days(window_days);
    let rows =
        sqlx::query("SELECT DISTINCT uuid, ip_hash FROM activity_ip WHERE day >= ? AND day < ?")
            .bind(first_day)
            .bind(cutoff)
            .fetch_all(pool)
            .await
            .context("failed to read IP hashes")?;
    rows.iter()
        .map(|row| Ok((row.try_get("uuid")?, row.try_get("ip_hash")?)))
        .collect()
}

/// `activity_meta`: recording start per data kind and heartbeat per worker.
pub async fn meta(pool: &MySqlPool) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query("SELECT name, value FROM activity_meta")
        .fetch_all(pool)
        .await
        .context("failed to read activity_meta")?;
    rows.iter()
        .map(|row| Ok((row.try_get("name")?, row.try_get("value")?)))
        .collect()
}

/// Problems that make a poll's data incomplete: a crystal or builder window
/// the plugin has not been recording for long enough.
pub fn coverage_problems(
    rules: &[(ClassCall, Rule)],
    meta: &HashMap<String, i64>,
    cutoff_ms: i64,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (call, rule) in rules {
        let (Some(key), Some(window)) = (rule.recording_key(), rule.window_days()) else {
            continue;
        };
        let needed = cutoff_ms - window * DAY_MS;
        match meta.get(key) {
            None => problems.push(format!(
                "{call}: the plugin has not started recording yet ({key} is missing)."
            )),
            Some(since) if *since > needed => problems.push(format!(
                "{call}: recording started {} but the {window}-day window needs data since {}.",
                date_text(*since),
                date_text(needed)
            )),
            Some(_) => {}
        }
    }
    problems
}

/// Softer problems, shown to staff next to a created poll: a worker that
/// stopped recording and alt detection that covers only part of its window.
pub fn coverage_warnings(
    meta: &HashMap<String, i64>,
    cutoff_ms: i64,
    now_ms: i64,
    identity_window_days: i64,
    ip_rows: usize,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut stale: Vec<&str> = meta
        .iter()
        .filter(|(name, beat)| name.starts_with("heartbeat.") && **beat < now_ms - 2 * 3_600_000)
        .map(|(name, _)| name.trim_start_matches("heartbeat."))
        .collect();
    stale.sort_unstable();
    if !stale.is_empty() {
        warnings.push(format!(
            "No heartbeat for over 2 hours from: {}. Recent data may be missing.",
            stale.join(", ")
        ));
    }
    let ip_needed = cutoff_ms - identity_window_days * DAY_MS;
    match meta.get("recording_since.ip") {
        _ if ip_rows == 0 => warnings.push(
            "No IP data for the alt-detection window: one person may hold several votes."
                .to_owned(),
        ),
        None => warnings
            .push("IP recording has no start time; alt detection may be partial.".to_owned()),
        Some(since) if *since > ip_needed => warnings.push(format!(
            "Alt detection covers only part of its {identity_window_days}-day window (recording since {}).",
            date_text(*since)
        )),
        Some(_) => {}
    }
    warnings
}

fn date_text(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms).map_or_else(
        || "?".to_owned(),
        |time| time.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

#[cfg(test)]
#[allow(unknown_lints, clippy::assert_is_empty)]
mod tests {
    use super::*;
    use crate::polls::config::test_config;

    const CUTOFF: i64 = 100 * DAY_MS;

    fn fight(a: &str, b: &str, started_at: i64, damage: i64) -> Fight {
        Fight {
            a: a.into(),
            b: b.into(),
            started_at,
            ended_at: started_at + 5_000,
            damage,
        }
    }

    fn judge_with(
        fights: &[Fight],
        config: &CrystalConfig,
        bots: &[&str],
        identity: &Identity,
        first_join: &HashMap<String, i64>,
    ) -> HashSet<String> {
        let bots: HashSet<String> = bots.iter().map(|bot| (*bot).to_owned()).collect();
        judge_crystal_fights(
            fights,
            config,
            &FightContext {
                cutoff_ms: CUTOFF,
                identity,
                bots: &bots,
                first_join,
            },
        )
    }

    fn everyone_old() -> HashMap<String, i64> {
        ["a", "b", "c", "d", "e", "f"]
            .into_iter()
            .map(|uuid| (uuid.to_owned(), 0))
            .collect()
    }

    fn judge(fights: &[Fight], config: &CrystalConfig, bots: &[&str]) -> HashSet<String> {
        judge_with(fights, config, bots, &Identity::default(), &everyone_old())
    }

    fn loose() -> CrystalConfig {
        CrystalConfig {
            window_days: 7,
            min_fights: 3,
            min_opponents: 2,
            min_damage: 10,
            pair_day_cap: 2,
            opponent_min_age_days: 3,
        }
    }

    fn day(n: i64) -> i64 {
        CUTOFF - n * DAY_MS
    }

    #[test]
    fn needs_enough_fights_against_enough_opponents() {
        let fights = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", day(3), 50),
        ];
        let result = judge(&fights, &loose(), &[]);
        assert!(result.contains("a"));
        assert!(!result.contains("b"), "b fought only once");

        let same = [
            fight("a", "b", day(1), 50),
            fight("a", "b", day(2), 50),
            fight("a", "b", day(3), 50),
        ];
        assert!(
            !judge(&same, &loose(), &[]).contains("a"),
            "one opponent is not enough"
        );
    }

    #[test]
    fn a_fight_must_end_before_the_cutoff() {
        let ends_after = Fight {
            ended_at: CUTOFF,
            ..fight("a", "d", CUTOFF - 1_000, 50)
        };
        let fights = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            ends_after,
        ];
        assert!(
            judge(&fights, &loose(), &[]).is_empty(),
            "ended at the cut-off: not counted"
        );

        let ends_before = Fight {
            ended_at: CUTOFF - 1,
            ..fight("a", "d", CUTOFF - 1_000, 50)
        };
        let fights = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            ends_before,
        ];
        assert!(
            judge(&fights, &loose(), &[]).contains("a"),
            "ended 1 ms earlier: counted"
        );

        let after = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", CUTOFF + 1, 50),
        ];
        assert!(
            judge(&after, &loose(), &[]).is_empty(),
            "newer data never counts"
        );
    }

    #[test]
    fn fights_before_the_window_do_not_count() {
        let fights = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", day(8), 50),
        ];
        assert!(judge(&fights, &loose(), &[]).is_empty());
        let edge = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", day(7), 50),
        ];
        assert!(
            judge(&edge, &loose(), &[]).contains("a"),
            "the window start is inclusive"
        );
    }

    #[test]
    fn small_fights_and_bot_fights_do_not_count() {
        let fights = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", day(3), 5),
        ];
        assert!(judge(&fights, &loose(), &[]).is_empty());
        let with_bot = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", day(3), 50),
        ];
        assert!(!judge(&with_bot, &loose(), &["d"]).contains("a"));
        assert!(!judge(&with_bot, &loose(), &["a"]).contains("a"));
    }

    #[test]
    fn a_pair_counts_at_most_the_daily_cap() {
        let config = CrystalConfig {
            min_fights: 3,
            min_opponents: 1,
            ..loose()
        };
        let same_day = CUTOFF - DAY_MS + 1_000;
        let mut fights: Vec<Fight> = (0..4).map(|n| fight("a", "b", same_day + n, 50)).collect();
        assert!(judge(&fights, &config, &[]).is_empty(), "capped at two");
        fights.push(fight("a", "b", day(3), 50));
        assert!(judge(&fights, &config, &[]).contains("a"));
    }

    #[test]
    fn opponents_who_shared_an_ip_are_not_real_opponents() {
        let fights = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", day(3), 50),
        ];
        // a and c joined from the same address: that fight is the same person.
        let identity = Identity::build(&[("a".into(), vec![1; 12]), ("c".into(), vec![1; 12])], 4);
        let result = judge_with(&fights, &loose(), &[], &identity, &everyone_old());
        assert!(!result.contains("a"));
        // On a crowded address nobody is linked.
        let crowd: Vec<(String, Vec<u8>)> = ["a", "c", "e", "f", "x"]
            .iter()
            .map(|uuid| ((*uuid).to_owned(), vec![1; 12]))
            .collect();
        let crowded = Identity::build(&crowd, 4);
        assert!(judge_with(&fights, &loose(), &[], &crowded, &everyone_old()).contains("a"));
    }

    #[test]
    fn young_or_unknown_opponents_are_ignored() {
        let fights = [
            fight("a", "b", day(1), 50),
            fight("a", "c", day(2), 50),
            fight("a", "d", day(3), 50),
        ];
        let identity = Identity::default();
        let mut young = everyone_old();
        young.insert("c".into(), CUTOFF - DAY_MS);
        assert!(!judge_with(&fights, &loose(), &[], &identity, &young).contains("a"));
        // Exactly at the minimum age is still too young (the contract uses a strict <).
        let mut edge = everyone_old();
        edge.insert("c".into(), CUTOFF - 3 * DAY_MS);
        assert!(!judge_with(&fights, &loose(), &[], &identity, &edge).contains("a"));
        edge.insert("c".into(), CUTOFF - 3 * DAY_MS - 1);
        assert!(judge_with(&fights, &loose(), &[], &identity, &edge).contains("a"));
        let mut missing = everyone_old();
        missing.remove("d");
        assert!(!judge_with(&fights, &loose(), &[], &identity, &missing).contains("a"));
    }

    #[test]
    fn overrides_win_over_config_and_are_recorded() {
        let config = test_config();
        let base = config.very_active.unwrap();
        let call = crate::polls::expr::parse("very_active(days=5, minutes=90)")
            .unwrap()
            .class_calls()
            .remove(0);
        let Rule::VeryActive(resolved) = Rule::resolve(&call, &config).unwrap() else {
            panic!("wrong rule");
        };
        assert_eq!((resolved.min_days, resolved.min_minutes), (5, 90));
        assert_eq!(resolved.window_days, base.window_days, "not overridden");
        let plain = Rule::resolve(&ClassCall::plain(Class::Veteran), &config).unwrap();
        assert_eq!(plain, Rule::Veteran(config.veteran.unwrap()));
        let json = serde_json::to_string(&plain).unwrap();
        assert!(json.contains("\"class\":\"veteran\""));
        assert_eq!(plain.window_days(), None);
        assert_eq!(
            Rule::resolve(&ClassCall::plain(Class::Active), &config)
                .unwrap()
                .window_days(),
            Some(config.active.unwrap().window_days)
        );
        let builder = crate::polls::expr::parse("builder(days=2, placed=100)")
            .unwrap()
            .class_calls()
            .remove(0);
        let Rule::Builder(resolved) = Rule::resolve(&builder, &config).unwrap() else {
            panic!("wrong rule");
        };
        assert_eq!((resolved.build_days, resolved.placed), (2, 100));
        assert_eq!(resolved.materials, config.builder.unwrap().materials);
    }

    #[test]
    fn a_class_without_settings_cannot_be_used() {
        let mut config = test_config();
        config.builder = None;
        let error = Rule::resolve(&ClassCall::plain(Class::Builder), &config).unwrap_err();
        assert!(error.contains("`builder` is not configured"));
        assert!(Rule::resolve(&ClassCall::plain(Class::Veteran), &config).is_ok());
    }

    #[test]
    fn the_cutoff_day_is_a_utc_day() {
        let cutoff = 1_791_295_200_000; // 2026-10-06 14:00 UTC
        assert_eq!(cutoff_date(cutoff).to_string(), "2026-10-06");
        assert_eq!(
            cutoff_date(cutoff - 14 * 3_600_000 - 1).to_string(),
            "2026-10-05"
        );
    }

    #[test]
    fn dashed_uuids_round_trip() {
        let plain = "aabbccdd000000000000000000000001";
        assert_eq!(dashed(plain), "aabbccdd-0000-0000-0000-000000000001");
        assert_eq!(normalize_uuid(&dashed(plain)), plain);
        assert_eq!(dashed("short"), "short");
    }

    fn rules(expression: &str) -> Vec<(ClassCall, Rule)> {
        let config = test_config();
        crate::polls::expr::parse(expression)
            .unwrap()
            .class_calls()
            .into_iter()
            .map(|call| {
                let rule = Rule::resolve(&call, &config).unwrap();
                (call, rule)
            })
            .collect()
    }

    #[test]
    fn coverage_requires_a_fully_recorded_window() {
        let cutoff = 100 * DAY_MS;
        let config = test_config();
        let crystal_days = config.crystal_pvper.unwrap().window_days;
        let builder_days = config.builder.unwrap().window_days;
        let mut meta = HashMap::new();
        // Only veteran and activity: nothing to check.
        assert!(coverage_problems(&rules("veteran AND active"), &meta, cutoff).is_empty());
        // Crystal with no recording start at all.
        assert_eq!(
            coverage_problems(&rules("crystal_pvper"), &meta, cutoff).len(),
            1
        );
        // One day short of the window is not enough.
        meta.insert(
            "recording_since.crystal".into(),
            cutoff - (crystal_days - 1) * DAY_MS,
        );
        assert_eq!(
            coverage_problems(&rules("crystal_pvper"), &meta, cutoff).len(),
            1
        );
        // Exactly the window is enough.
        meta.insert(
            "recording_since.crystal".into(),
            cutoff - crystal_days * DAY_MS,
        );
        assert!(coverage_problems(&rules("crystal_pvper"), &meta, cutoff).is_empty());
        // Builder reads its own key and has its own window.
        assert_eq!(coverage_problems(&rules("builder"), &meta, cutoff).len(), 1);
        meta.insert(
            "recording_since.build".into(),
            cutoff - builder_days * DAY_MS,
        );
        assert!(coverage_problems(&rules("crystal_pvper OR builder"), &meta, cutoff).is_empty());
    }

    #[test]
    fn warnings_cover_stale_workers_and_partial_alt_detection() {
        let now = 100 * DAY_MS;
        let mut meta = HashMap::new();
        meta.insert("recording_since.ip".into(), now - 40 * DAY_MS);
        meta.insert("heartbeat.worker-0".into(), now - 60_000);
        meta.insert("heartbeat.worker-1".into(), now - 3 * 3_600_000);
        let warnings = coverage_warnings(&meta, now, now, 30, 10);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("worker-1") && !warnings[0].contains("worker-0"));

        meta.insert("recording_since.ip".into(), now - 10 * DAY_MS);
        let warnings = coverage_warnings(&meta, now, now, 30, 10);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("part of its 30-day window"))
        );

        let warnings = coverage_warnings(&HashMap::new(), now, now, 30, 0);
        assert_eq!(warnings.len(), 1, "no rows at all is one warning");
    }
}
