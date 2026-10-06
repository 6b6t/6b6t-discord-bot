//! The poll service: create (freeze), vote, refresh, close.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use anyhow::{Context as _, Result};
use chrono::{Duration, NaiveDate, TimeZone as _, Utc};
use poise::serenity_prelude as serenity;
use sqlx::{MySqlPool, Row as _};
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::{
    classes::{self, EvalContext, Rule, cutoff_date},
    config::PollConfig,
    expr::{self, ClassCall, Expr},
    identity::{self, Candidate, Identity, Voter},
    render::{self, PollView},
    store::{NewPoll, PollRow, PollStore, Status, VoteOutcome},
    transport::{DiscordTransport, EditRequest, EditResult, PollTransport},
};
use crate::database::normalize_uuid;

/// A worker tick runs every 30 seconds; a poll message is edited at most once
/// per this many milliseconds (Discord allows about 5 edits per 5 seconds per
/// channel, so one poll can never come close).
const MIN_RENDER_GAP_MS: i64 = 25_000;
/// An open poll whose message was never posted is cancelled after this long.
const ORPHAN_SECONDS: i64 = 300;
/// The voter-row retention check runs every this many worker ticks (hourly).
const PURGE_EVERY_TICKS: u64 = 120;

#[derive(Clone)]
pub struct PollService {
    store: PollStore,
    link: MySqlPool,
    stats: MySqlPool,
    config: Arc<PollConfig>,
    initialized: Arc<AtomicBool>,
    worker_lock: Arc<Mutex<()>>,
    /// One lock per poll that is being redrawn or closed. See [`Self::lock_poll`].
    poll_locks: Arc<std::sync::Mutex<HashMap<u64, Arc<Mutex<()>>>>>,
    ticks: Arc<AtomicU64>,
}

/// What staff typed into `/poll create`.
#[derive(Clone, Debug)]
pub struct CreateRequest {
    pub title: String,
    pub options: Vec<String>,
    pub requires: String,
    pub duration_seconds: i64,
    pub channel_id: serenity::ChannelId,
    pub created_by: serenity::UserId,
}

/// The frozen eligible set plus what was recorded about how it was computed.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub voters: Vec<Voter>,
    pub rule_json: String,
    pub window_days: i32,
}

/// A validated request, ready to store and post.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub new_poll: NewPoll,
    pub voters: Vec<Voter>,
    /// Staff-only notes: partial data, stale workers, alt detection gaps.
    pub warnings: Vec<String>,
}

/// What the `PlayerActivity` plugin recorded, read once per poll creation.
/// Every table is read on its own: one that is missing, or that the bot's stats
/// user may not read, leaves the other tables' data intact.
#[derive(Clone, Debug, Default)]
pub struct PluginData {
    /// The plugin tables the stats user can see.
    present: HashSet<String>,
    bots: HashSet<String>,
    observations: Vec<(String, Vec<u8>)>,
    meta: HashMap<String, i64>,
}

impl PluginData {
    fn has(&self, table: &str) -> bool {
        self.present.contains(table)
    }
}

/// Marks a link whose `created_at` is NULL. Only used to order alts.
const LINK_TIME_UNKNOWN: i64 = i64::MAX;
/// Reading the snapshot is retried this many times when UTC midnight passes in
/// the middle of it (see [`PollService::seal_with`]).
const SEAL_ATTEMPTS: usize = 3;

/// Milliseconds since the epoch. Injected so tests can fix the cut-off.
pub type Clock = dyn Fn() -> i64 + Send + Sync;

fn system_clock() -> i64 {
    Utc::now().timestamp_millis()
}

/// A validated `/poll create` request: the parsed expression and its resolved
/// rules. Nothing has been read from the database yet.
#[derive(Debug)]
pub struct Validated {
    expression: Expr,
    rules: Vec<(ClassCall, Rule)>,
}

/// The creation snapshot's boundary. Everything that has no usable timestamp
/// (account links, the day-granular IP evidence, bot flags, the coverage
/// table) is read first; only then is the cut-off fixed. So every row in here
/// existed before the cut-off, and nothing written later can enter, however
/// long the class queries that follow take. Rows that do carry a timestamp
/// (fights, play time, build days) are read afterwards and filtered by it.
#[derive(Clone, Debug)]
pub struct Sealed {
    cutoff_ms: i64,
    links: Vec<LinkRow>,
    plugin: PluginData,
}

fn resolve_rules(
    expression: &Expr,
    config: &PollConfig,
) -> std::result::Result<Vec<(ClassCall, Rule)>, String> {
    expression
        .class_calls()
        .into_iter()
        .map(|call| Rule::resolve(&call, config).map(|rule| (call, rule)))
        .collect()
}

pub enum CreateOutcome {
    Created {
        poll_id: u64,
        message: serenity::MessageId,
        channel: serenity::ChannelId,
        /// Present only when the turnout toggle is on.
        eligible: Option<usize>,
        /// Staff-only notes about the data behind the snapshot.
        warnings: Vec<String>,
    },
    /// A problem staff can fix; nothing was stored or posted.
    Rejected(String),
}

pub enum CloseOutcome {
    Closed { text: String },
    NotOpen(String),
}

impl PollService {
    pub fn new(link: MySqlPool, stats: MySqlPool, config: PollConfig) -> Self {
        Self {
            store: PollStore::new(link.clone()),
            link,
            stats,
            config: Arc::new(config),
            initialized: Arc::default(),
            worker_lock: Arc::default(),
            poll_locks: Arc::default(),
            ticks: Arc::default(),
        }
    }

    #[cfg(test)]
    pub fn store_for_tests(&self) -> &PollStore {
        &self.store
    }

    /// Creates the poll tables. Votes are refused until this has succeeded.
    pub async fn ready(&self) -> Result<()> {
        self.store.migrate().await?;
        self.initialized.store(true, Ordering::Release);
        tracing::info!("eligibility polls are ready");
        Ok(())
    }

    // ---- create ------------------------------------------------------

    /// Validates the request, fixes the cut-off and computes the frozen
    /// snapshot. Nothing is stored; a bad request comes back as `Err(message)`
    /// for staff.
    pub async fn prepare(
        &self,
        request: &CreateRequest,
    ) -> Result<std::result::Result<Prepared, String>> {
        self.prepare_with(request, &system_clock).await
    }

    /// [`Self::prepare`] with a fixed cut-off, for tests.
    #[cfg(test)]
    pub async fn prepare_at(
        &self,
        request: &CreateRequest,
        cutoff_ms: i64,
    ) -> Result<std::result::Result<Prepared, String>> {
        self.prepare_with(request, &move || cutoff_ms).await
    }

    pub async fn prepare_with(
        &self,
        request: &CreateRequest,
        clock: &Clock,
    ) -> Result<std::result::Result<Prepared, String>> {
        let validated = match self.validate(request) {
            Ok(validated) => validated,
            Err(message) => return Ok(Err(message)),
        };
        let sealed = self.seal_with(clock).await?;
        self.prepare_sealed(request, &validated, sealed).await
    }

    /// Everything that can be refused without touching the database.
    pub fn validate(&self, request: &CreateRequest) -> std::result::Result<Validated, String> {
        render::validate_poll_text(&request.title, &request.options)?;
        let min = self.config.min_duration_minutes * 60;
        let max = self.config.max_duration_days * 86_400;
        if request.duration_seconds < min || request.duration_seconds > max {
            return Err(format!(
                "The duration must be between {} minutes and {} days.",
                self.config.min_duration_minutes, self.config.max_duration_days
            ));
        }
        let expression =
            expr::parse(&request.requires).map_err(|error| render::requires_error(&error))?;
        let rules = resolve_rules(&expression, &self.config)?;
        Ok(Validated { expression, rules })
    }

    /// Reads the sources that carry no usable timestamp, then fixes the
    /// cut-off (see [`Sealed`]). The IP window ends on the cut-off day, so the
    /// day is taken from the clock before the reads and checked again after:
    /// if UTC midnight passed in between, the reads are repeated.
    pub async fn seal_with(&self, clock: &Clock) -> Result<Sealed> {
        for _ in 0..SEAL_ATTEMPTS {
            let day = cutoff_date(clock());
            let links = self.links().await?;
            let plugin = self.plugin_data(day).await?;
            let cutoff_ms = clock();
            if cutoff_date(cutoff_ms) == day {
                return Ok(Sealed {
                    cutoff_ms,
                    links,
                    plugin,
                });
            }
            tracing::info!("UTC midnight passed while the poll snapshot was read; reading again");
        }
        anyhow::bail!("could not read a consistent poll snapshot around UTC midnight")
    }

    /// Checks the data the poll needs and evaluates the frozen snapshot.
    pub async fn prepare_sealed(
        &self,
        request: &CreateRequest,
        validated: &Validated,
        sealed: Sealed,
    ) -> Result<std::result::Result<Prepared, String>> {
        let cutoff_ms = sealed.cutoff_ms;
        let data = &sealed.plugin;
        // Bot-marked accounts must never count, and the only record of them is
        // the plugin's table. Without it the poll is refused, not warned about.
        if !data.has(classes::TABLE_PLAYER) {
            return Ok(Err(format!(
                "The bot flags cannot be read (`{}` is missing, or the bot's stats user may not read it), so no poll can run: bot-marked accounts must never count. Deploy the PlayerActivity plugin and check the grants.",
                classes::TABLE_PLAYER
            )));
        }
        for (call, rule) in &validated.rules {
            let missing: Vec<&str> = rule
                .required_tables()
                .iter()
                .copied()
                .filter(|table| !data.has(table))
                .collect();
            if !missing.is_empty() {
                return Ok(Err(format!(
                    "`{call}` cannot run: the PlayerActivity table(s) {} are missing, or the bot's stats user may not read them.",
                    missing.join(", ")
                )));
            }
        }
        let mut warnings = Vec::new();
        if data.has(classes::TABLE_META) {
            let problems = classes::coverage_problems(&validated.rules, &data.meta, cutoff_ms);
            if !problems.is_empty() {
                if self.config.allow_partial_data {
                    warnings.extend(problems);
                } else {
                    return Ok(Err(format!(
                        "The recorded data does not cover the whole window, so nothing was posted:\n- {}",
                        problems.join("\n- ")
                    )));
                }
            }
        }
        warnings.extend(self.plugin_warnings(data, cutoff_ms));
        let snapshot = self
            .evaluate_snapshot(
                &validated.expression,
                &request.requires,
                &validated.rules,
                &sealed,
            )
            .await?;
        if snapshot.voters.is_empty() {
            return Ok(Err(
                "Nobody is eligible under that expression right now, so nothing was posted.".into(),
            ));
        }
        Ok(Ok(Prepared {
            new_poll: NewPoll {
                channel_id: request.channel_id.to_string(),
                title: request.title.trim().to_owned(),
                options: request
                    .options
                    .iter()
                    .map(|option| option.trim().to_owned())
                    .collect(),
                requires_expr: request.requires.trim().to_owned(),
                rule_json: snapshot.rule_json,
                cutoff_ms,
                window_days: snapshot.window_days,
                duration_seconds: request.duration_seconds,
                created_by: request.created_by.to_string(),
            },
            voters: snapshot.voters,
            warnings,
        }))
    }

    /// Staff-only notes about plugin data that is missing or thin. None of
    /// them stops a poll: a missing bot-flag table does, and is handled before.
    fn plugin_warnings(&self, data: &PluginData, cutoff_ms: i64) -> Vec<String> {
        let mut warnings = Vec::new();
        if !data.has(classes::TABLE_IP) {
            warnings.push(format!(
                "`{}` is missing or not readable: alt detection is off, so one person may hold several votes.",
                classes::TABLE_IP
            ));
        }
        if !data.has(classes::TABLE_META) {
            warnings.push(format!(
                "`{}` is missing or not readable: the recording coverage and the workers' heartbeats could not be checked.",
                classes::TABLE_META
            ));
        }
        if data.has(classes::TABLE_IP) && data.has(classes::TABLE_META) {
            warnings.extend(classes::coverage_warnings(
                &data.meta,
                cutoff_ms,
                system_clock(),
                self.config.identity.window_days,
                data.observations.len(),
            ));
        } else if data.has(classes::TABLE_IP) && data.observations.is_empty() {
            warnings.push(
                "No IP data for the alt-detection window: one person may hold several votes."
                    .to_owned(),
            );
        }
        warnings
    }

    /// Reads what the `PlayerActivity` plugin recorded that every poll needs:
    /// bot flags, IP hashes and the coverage table. Each table is read if it is
    /// there, whatever the others do; [`Self::prepare_sealed`] decides what a
    /// missing one means for the poll.
    async fn plugin_data(&self, cutoff_day: NaiveDate) -> Result<PluginData> {
        let present = classes::available_plugin_tables(&self.stats).await?;
        let mut data = PluginData::default();
        if present.contains(classes::TABLE_PLAYER) {
            data.bots = classes::bot_accounts(&self.stats).await?;
        }
        if present.contains(classes::TABLE_IP) {
            data.observations =
                classes::ip_observations(&self.stats, cutoff_day, self.config.identity.window_days)
                    .await?;
        }
        if present.contains(classes::TABLE_META) {
            data.meta = classes::meta(&self.stats).await?;
        }
        data.present = present;
        Ok(data)
    }

    /// Evaluates the expression against a sealed snapshot of the data, then
    /// keeps one linked Discord account per person. Used by the class tests.
    #[cfg(test)]
    pub async fn build_snapshot(
        &self,
        expression: &Expr,
        expression_text: &str,
        cutoff_ms: i64,
    ) -> Result<Snapshot> {
        let rules = resolve_rules(expression, &self.config).map_err(anyhow::Error::msg)?;
        let sealed = self.seal_with(&move || cutoff_ms).await?;
        self.evaluate_snapshot(expression, expression_text, &rules, &sealed)
            .await
    }

    /// The eligible set from data before the cut-off. The result is the only
    /// thing the poll ever uses to decide who may vote.
    async fn evaluate_snapshot(
        &self,
        expression: &Expr,
        expression_text: &str,
        rules: &[(ClassCall, Rule)],
        sealed: &Sealed,
    ) -> Result<Snapshot> {
        let cutoff_ms = sealed.cutoff_ms;
        // A link made at or after the cut-off second cannot be shown to
        // predate it. The read already happened before the cut-off was fixed;
        // this keeps that true when the cut-off is injected or the clocks differ.
        let cutoff_seconds = cutoff_ms.div_euclid(1000);
        let links: Vec<&LinkRow> = sealed
            .links
            .iter()
            .filter(|link| link.linked_at == LINK_TIME_UNKNOWN || link.linked_at < cutoff_seconds)
            .collect();
        let bots = &sealed.plugin.bots;
        let identity = Identity::build(
            &sealed.plugin.observations,
            usize::try_from(self.config.identity.hub_limit).unwrap_or(usize::MAX),
        );

        let mut sets: HashMap<ClassCall, HashSet<String>> = HashMap::new();
        for (call, rule) in rules {
            let members = classes::evaluate(
                rule,
                &EvalContext {
                    stats: &self.stats,
                    cutoff_ms,
                    identity: &identity,
                    bots,
                },
            )
            .await
            .with_context(|| format!("failed to evaluate {call}"))?;
            sets.insert(call.clone(), members);
        }

        let linked_humans: HashSet<String> = links
            .iter()
            .map(|link| normalize_uuid(&link.uuid))
            .filter(|uuid| !bots.contains(uuid))
            .collect();
        let universe = if expression.uses_not() {
            linked_humans.clone()
        } else {
            HashSet::new()
        };
        let members = expression.evaluate(&sets, &universe);
        let candidates: Vec<Candidate> = links
            .into_iter()
            .filter(|link| {
                let uuid = normalize_uuid(&link.uuid);
                linked_humans.contains(&uuid) && members.contains(&uuid)
            })
            .map(|link| Candidate {
                discord_id: link.discord_id.clone(),
                uuid: link.uuid.clone(),
                linked_at: link.linked_at,
            })
            .collect();
        let voters = identity::pick_voters(candidates, &identity);

        let window_days = rules
            .iter()
            .filter_map(|(_, rule)| rule.window_days())
            .max()
            .unwrap_or(0);
        let rule_json = serde_json::json!({
            "requires": expression_text.trim(),
            "cutoff_ms": cutoff_ms,
            "rules": rules
                .iter()
                .map(|(call, rule)| serde_json::json!({"call": call.to_string(), "rule": rule}))
                .collect::<Vec<_>>(),
            "identity": &self.config.identity,
        })
        .to_string();
        Ok(Snapshot {
            voters,
            rule_json,
            window_days: i32::try_from(window_days).unwrap_or(i32::MAX),
        })
    }

    async fn links(&self) -> Result<Vec<LinkRow>> {
        let rows = sqlx::query(
            "SELECT uuid, discord_id, COALESCE(CAST(UNIX_TIMESTAMP(created_at) AS SIGNED), 9223372036854775807) AS linked_at FROM uuid_to_discord",
        )
        .fetch_all(&self.link)
        .await
        .context("failed to read account links")?;
        rows.iter()
            .map(|row| {
                Ok(LinkRow {
                    uuid: row.try_get("uuid")?,
                    discord_id: row.try_get("discord_id")?,
                    linked_at: row.try_get("linked_at")?,
                })
            })
            .collect()
    }

    /// Freezes, stores and posts a poll.
    pub async fn create(
        &self,
        ctx: &serenity::Context,
        request: &CreateRequest,
    ) -> Result<CreateOutcome> {
        if !self.initialized.load(Ordering::Acquire) {
            return Ok(CreateOutcome::Rejected(
                "Polls are still starting. Try again in a moment.".into(),
            ));
        }
        let prepared = match self.prepare(request).await? {
            Ok(prepared) => prepared,
            Err(message) => return Ok(CreateOutcome::Rejected(message)),
        };
        let eligible = prepared.voters.len();
        let warnings = prepared.warnings.clone();
        let poll_id = self
            .store
            .create_poll(&prepared.new_poll, &prepared.voters)
            .await?;
        let Some(row) = self.store.poll(poll_id).await? else {
            anyhow::bail!("poll {poll_id} disappeared after it was stored");
        };
        let options = row.options()?;
        let zero_counts = self.config.show_live_counts.then(|| vec![0; options.len()]);
        let sent = request
            .channel_id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .embed(self.embed_for(&row, zero_counts.as_deref(), false)?)
                    .components(render::buttons(poll_id, &options))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await;
        match sent {
            Ok(message) => {
                self.store
                    .attach_message(poll_id, &message.id.to_string())
                    .await?;
                tracing::info!(poll_id, eligible, "poll created");
                Ok(CreateOutcome::Created {
                    poll_id,
                    message: message.id,
                    channel: message.channel_id,
                    eligible: self.config.show_turnout.then_some(eligible),
                    warnings,
                })
            }
            Err(error) => {
                self.store.cancel(poll_id).await?;
                tracing::error!(%error, poll_id, "failed to post the poll message");
                Ok(CreateOutcome::Rejected(format!(
                    "I could not post in <#{}>: {error}. The poll was cancelled.",
                    request.channel_id
                )))
            }
        }
    }

    // ---- voting ------------------------------------------------------

    pub async fn handle_component(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ComponentInteraction,
    ) -> Result<bool> {
        let Some((poll_id, option)) = render::parse_button(&interaction.data.custom_id) else {
            return Ok(false);
        };
        // Discord wants an answer within three seconds: defer privately, then
        // do the database work and edit the reply. Always answer, even on error.
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Defer(
                    serenity::CreateInteractionResponseMessage::new().ephemeral(true),
                ),
            )
            .await?;
        let content = if self.initialized.load(Ordering::Acquire) {
            match self
                .process_vote(poll_id, &interaction.user.id.to_string(), option)
                .await
            {
                Ok(text) => text,
                Err(error) => {
                    tracing::error!(%error, poll_id, "failed to process a poll vote");
                    render::ERROR_REPLY.to_owned()
                }
            }
        } else {
            render::STARTING_REPLY.to_owned()
        };
        interaction
            .edit_response(
                ctx,
                serenity::EditInteractionResponse::new().content(content),
            )
            .await?;
        Ok(true)
    }

    /// Counts or rejects one click and returns the private reply text. Only the
    /// frozen snapshot is consulted: no class data and no live link table.
    pub async fn process_vote(
        &self,
        poll_id: u64,
        discord_id: &str,
        option: usize,
    ) -> Result<String> {
        let Some(row) = self.store.poll(poll_id).await? else {
            return Ok(render::MISSING_REPLY.into());
        };
        let options = row.options()?;
        if row.status() != Status::Open {
            self.store
                .record_denial(poll_id, render::DENIAL_CLOSED)
                .await?;
            return Ok(render::CLOSED_REPLY.into());
        }
        let outcome = self
            .store
            .cast_vote(poll_id, discord_id, option, options.len())
            .await?;
        Ok(match outcome {
            VoteOutcome::Counted { first, changed } => {
                render::vote_reply(&options[option], first, changed)
            }
            VoteOutcome::NotEligible => {
                self.store
                    .record_denial(poll_id, render::DENIAL_NOT_ELIGIBLE)
                    .await?;
                render::ineligible_reason(&self.who_for(&row))
            }
            VoteOutcome::Closed => {
                self.store
                    .record_denial(poll_id, render::DENIAL_CLOSED)
                    .await?;
                render::CLOSED_REPLY.into()
            }
            VoteOutcome::Missing => render::MISSING_REPLY.into(),
            VoteOutcome::InvalidOption => render::ERROR_REPLY.into(),
        })
    }

    fn who_for(&self, row: &PollRow) -> String {
        expr::parse(&row.requires_expr).map_or_else(
            |_| "recent activity on 6b6t".to_owned(),
            |parsed| render::who_phrase(&parsed, &self.config.texts),
        )
    }

    // ---- rendering ---------------------------------------------------

    fn embed_for(
        &self,
        row: &PollRow,
        counts: Option<&[u32]>,
        closed: bool,
    ) -> Result<serenity::CreateEmbed> {
        let options = row.options()?;
        let who = self.who_for(row);
        let cutoff_seconds = row.cutoff_ms.div_euclid(1000);
        let window_start = (row.window_days > 0).then(|| {
            let first_day = cutoff_date(row.cutoff_ms) - Duration::days(i64::from(row.window_days));
            Utc.from_utc_datetime(&first_day.and_hms_opt(0, 0, 0).unwrap_or_default())
                .timestamp()
        });
        let view = PollView {
            poll_id: row.poll_id,
            title: &row.title,
            options: &options,
            who: &who,
            window_start,
            cutoff_seconds,
            ends_at: row.ends_at,
            closed,
            counts,
            eligible: self.config.show_turnout.then_some(row.eligible_count),
        };
        Ok(render::embed(&view))
    }

    async fn edit_message(
        &self,
        transport: &dyn PollTransport,
        row: &PollRow,
        counts: Option<&[u32]>,
        closed: bool,
    ) -> Result<EditResult> {
        let (Some(message_id), Ok(channel_id)) = (
            row.message_id
                .as_deref()
                .and_then(|id| id.parse::<u64>().ok()),
            row.channel_id.parse::<u64>(),
        ) else {
            return Ok(EditResult::Gone);
        };
        let components = if closed {
            Vec::new()
        } else {
            render::buttons(row.poll_id, &row.options()?)
        };
        let edit = serenity::EditMessage::new()
            .embed(self.embed_for(row, counts, closed)?)
            .components(components);
        transport
            .edit_message(EditRequest {
                channel_id,
                message_id,
                closed,
                counts: counts.map(<[u32]>::to_vec),
                edit,
            })
            .await
    }

    /// Redraws an open poll with the current counts.
    async fn refresh(&self, transport: &dyn PollTransport, row: &PollRow) -> Result<()> {
        let options = row.options()?;
        let counts = if self.config.show_live_counts {
            Some(render::tally(
                options.len(),
                &self.store.votes(row.poll_id).await?,
                &HashSet::new(),
            ))
        } else {
            None
        };
        if self
            .edit_message(transport, row, counts.as_deref(), false)
            .await?
            == EditResult::Gone
        {
            tracing::warn!(
                poll_id = row.poll_id,
                "poll message is gone; cancelling the poll"
            );
            self.store.cancel(row.poll_id).await?;
        }
        Ok(())
    }

    /// The lock that orders everything touching one poll's message: the open
    /// redraw, the final edit and the stored result. Without it a redraw that
    /// loaded the poll while it was open can land after `/poll close` has
    /// edited the closed message, and the worker and `/poll close` can both
    /// finalize. A lock is held only while a poll is being edited; idle ones
    /// are dropped here. Polls are served by one bot process (the worker is
    /// serialized in-process as well), so a process-local lock is enough, and
    /// the stored result is still written only once (`PollStore::finalize`).
    pub(super) async fn lock_poll(&self, poll_id: u64) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self
                .poll_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            Arc::clone(locks.entry(poll_id).or_default())
        };
        lock.lock_owned().await
    }

    #[cfg(test)]
    fn tracked_poll_locks(&self) -> usize {
        self.poll_locks.lock().map_or(0, |locks| locks.len())
    }

    // ---- closing -----------------------------------------------------

    /// Computes the final counts from the frozen snapshot, edits the message
    /// and stores the result. Eligibility is not re-evaluated: only integrity
    /// facts (still in the server) are applied. Returns `None` when there is
    /// nothing to do: the poll is not closed, or somebody finalized it first.
    /// The poll is re-read under [`Self::lock_poll`], so two callers can never
    /// both pass the "not finalized yet" check.
    async fn finalize(
        &self,
        transport: &dyn PollTransport,
        poll_id: u64,
    ) -> Result<Option<Vec<u32>>> {
        let _guard = self.lock_poll(poll_id).await;
        let Some(row) = self.store.poll(poll_id).await? else {
            return Ok(None);
        };
        if row.status() != Status::Closed || row.finalized != 0 {
            return Ok(None);
        }
        let options = row.options()?;
        let votes = self.store.votes(poll_id).await?;
        let excluded = if self.config.drop_departed_voters {
            Self::departed(transport, &votes).await
        } else {
            HashSet::new()
        };
        let counts = render::tally(options.len(), &votes, &excluded);
        let result = serde_json::json!({
            "counts": counts,
            "total": counts.iter().sum::<u32>(),
            "left_server": excluded.len(),
        })
        .to_string();
        self.edit_message(transport, &row, Some(&counts), true)
            .await?;
        if !self.store.finalize(poll_id, &result).await? {
            tracing::warn!(poll_id, "another process finalized the poll first");
            return Ok(None);
        }
        tracing::info!(poll_id, "poll closed");
        Ok(Some(counts))
    }

    async fn departed(transport: &dyn PollTransport, votes: &[(String, u8)]) -> HashSet<String> {
        let mut gone = HashSet::new();
        for (voter, _) in votes {
            let Ok(user_id) = voter.parse::<u64>() else {
                continue;
            };
            if transport.member_gone(user_id).await {
                gone.insert(voter.clone());
            }
        }
        gone
    }

    /// `/poll close`: closes now, whatever the end time says.
    pub async fn close(&self, ctx: &serenity::Context, poll_id: u64) -> Result<CloseOutcome> {
        self.close_with(&DiscordTransport::new(ctx), poll_id).await
    }

    pub async fn close_with(
        &self,
        transport: &dyn PollTransport,
        poll_id: u64,
    ) -> Result<CloseOutcome> {
        let Some(row) = self.store.poll(poll_id).await? else {
            return Ok(CloseOutcome::NotOpen(render::MISSING_REPLY.into()));
        };
        if !self.store.claim_close(poll_id, true).await? {
            return Ok(CloseOutcome::NotOpen(
                "That poll is not open (it already closed or was cancelled).".into(),
            ));
        }
        let text = if let Some(counts) = self.finalize(transport, poll_id).await? {
            render::results_text(
                &row.title,
                &row.options()?,
                &counts,
                self.config.show_turnout.then_some(row.eligible_count),
                &self.denials_for_staff(poll_id).await?,
                true,
            )
        } else {
            // The worker saw the claim and finalized first: its stored result
            // is the final one.
            self.results(poll_id).await?.unwrap_or_else(|| {
                "The poll is closed; the final message update will retry shortly.".into()
            })
        };
        Ok(CloseOutcome::Closed { text })
    }

    /// `/poll results`: counts of eligible votes only.
    pub async fn results(&self, poll_id: u64) -> Result<Option<String>> {
        let Some(row) = self.store.poll(poll_id).await? else {
            return Ok(None);
        };
        let options = row.options()?;
        let (counts, closed) = if let Some(result) = &row.result_json {
            let parsed: serde_json::Value = serde_json::from_str(result)?;
            let counts = parsed["counts"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .map(|value| u32::try_from(value.as_u64().unwrap_or(0)).unwrap_or(0))
                        .collect()
                })
                .unwrap_or_default();
            (counts, true)
        } else {
            (
                render::tally(
                    options.len(),
                    &self.store.votes(poll_id).await?,
                    &HashSet::new(),
                ),
                row.status() != Status::Open,
            )
        };
        Ok(Some(render::results_text(
            &row.title,
            &options,
            &counts,
            self.config.show_turnout.then_some(row.eligible_count),
            &self.denials_for_staff(poll_id).await?,
            closed,
        )))
    }

    /// Refused-click counters are shown to staff only with the turnout toggle,
    /// because they hint at how many people were eligible.
    async fn denials_for_staff(&self, poll_id: u64) -> Result<Vec<(String, u32)>> {
        if self.config.show_turnout {
            self.store.denials(poll_id).await
        } else {
            Ok(Vec::new())
        }
    }

    /// A deleted poll message ends the poll: nobody can vote on it any more.
    pub async fn on_delete(
        &self,
        channel_id: serenity::ChannelId,
        message_id: serenity::MessageId,
    ) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }
        match self
            .store
            .cancel_by_message(&channel_id.to_string(), &message_id.to_string())
            .await
        {
            Ok(0) => {}
            Ok(_) => tracing::warn!(%message_id, "poll message was deleted; poll cancelled"),
            Err(error) => {
                tracing::error!(%error, "failed to cancel a poll whose message was deleted");
            }
        }
    }

    // ---- worker ------------------------------------------------------

    /// Runs every 30 seconds: cancels orphans, closes due polls, finishes
    /// closes that were interrupted, refreshes changed counts, trims old rows.
    pub async fn worker(&self, ctx: &serenity::Context) {
        self.worker_with(&DiscordTransport::new(ctx)).await;
    }

    pub async fn worker_with(&self, transport: &dyn PollTransport) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }
        let Ok(_guard) = self.worker_lock.try_lock() else {
            return;
        };
        match self.store.orphaned(ORPHAN_SECONDS).await {
            Ok(ids) => {
                for id in ids {
                    tracing::warn!(poll_id = id, "cancelling a poll that was never posted");
                    if let Err(error) = self.store.cancel(id).await {
                        tracing::error!(%error, poll_id = id, "failed to cancel an orphaned poll");
                    }
                }
            }
            Err(error) => tracing::error!(%error, "failed to look for orphaned polls"),
        }
        match self.store.due().await {
            Ok(ids) => {
                for id in ids {
                    if let Err(error) = self.store.claim_close(id, false).await {
                        tracing::error!(%error, poll_id = id, "failed to close a due poll");
                    }
                }
            }
            Err(error) => tracing::error!(%error, "failed to load due polls"),
        }
        match self.store.unfinalized().await {
            Ok(ids) => {
                for id in ids {
                    if let Err(error) = self.finalize(transport, id).await {
                        tracing::error!(%error, poll_id = id, "failed to finalize a poll; will retry");
                    }
                }
            }
            Err(error) => tracing::error!(%error, "failed to load polls to finalize"),
        }
        match self.store.dirty(MIN_RENDER_GAP_MS).await {
            Ok(ids) => {
                for id in ids {
                    if let Err(error) = self.refresh_one(transport, id).await {
                        tracing::error!(%error, poll_id = id, "failed to refresh a poll message; will retry");
                        let _ = self.store.mark_dirty(id).await;
                    }
                }
            }
            Err(error) => tracing::error!(%error, "failed to load polls to refresh"),
        }
        if self.ticks.fetch_add(1, Ordering::Relaxed) % PURGE_EVERY_TICKS == PURGE_EVERY_TICKS - 1 {
            match self
                .store
                .purge_voter_rows(self.config.snapshot_retention_days)
                .await
            {
                Ok(0) => {}
                Ok(rows) => tracing::info!(rows, "removed voter rows of old polls"),
                Err(error) => tracing::error!(%error, "failed to remove voter rows of old polls"),
            }
        }
    }

    /// Redraws one poll whose counts changed. Everything happens under the
    /// poll's lock and the poll is read after taking it, so a redraw that
    /// started before `/poll close` finishes first and one that starts after it
    /// sees a closed poll and does nothing.
    pub async fn refresh_one(&self, transport: &dyn PollTransport, poll_id: u64) -> Result<()> {
        let _guard = self.lock_poll(poll_id).await;
        if !self.store.claim_render(poll_id).await? {
            return Ok(());
        }
        let Some(row) = self.store.poll(poll_id).await? else {
            return Ok(());
        };
        if row.status() != Status::Open {
            return Ok(());
        }
        self.refresh(transport, &row).await
    }
}

#[derive(Clone, Debug)]
struct LinkRow {
    uuid: String,
    discord_id: String,
    linked_at: i64,
}

/// Parses durations such as `90m`, `12h`, `3d` or `1d12h`.
pub fn parse_duration(input: &str) -> Option<i64> {
    let mut total: i64 = 0;
    let mut number = String::new();
    let mut parts = 0;
    for character in input.trim().chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        let value: i64 = number.parse().ok()?;
        number.clear();
        let unit = match character.to_ascii_lowercase() {
            'm' => 60,
            'h' => 3_600,
            'd' => 86_400,
            _ => return None,
        };
        total = total.checked_add(value.checked_mul(unit)?)?;
        parts += 1;
    }
    (number.is_empty() && parts > 0 && total > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polls::config::test_config;

    #[tokio::test]
    async fn poll_locks_are_exclusive_per_poll_and_idle_ones_are_dropped() {
        let pool = MySqlPool::connect_lazy("mysql://nobody:nothing@127.0.0.1:1/none").unwrap();
        let service = PollService::new(pool.clone(), pool, test_config());
        let first = service.lock_poll(1).await;
        // Another poll is not blocked by the first one's lock.
        let second = service.lock_poll(2).await;
        assert_eq!(service.tracked_poll_locks(), 2);
        // The same poll is: a second taker waits until the first is released.
        let waiting = tokio::spawn({
            let service = service.clone();
            async move { service.lock_poll(1).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!waiting.is_finished());
        drop(first);
        let again = waiting.await.unwrap();
        drop(again);
        drop(second);
        // Idle locks are forgotten the next time anybody asks for one.
        let third = service.lock_poll(3).await;
        assert_eq!(service.tracked_poll_locks(), 1);
        drop(third);
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("90m"), Some(5_400));
        assert_eq!(parse_duration("12h"), Some(43_200));
        assert_eq!(parse_duration("3d"), Some(259_200));
        assert_eq!(parse_duration("1d12h"), Some(129_600));
        assert_eq!(parse_duration(" 2D "), Some(172_800));
        for bad in ["", "d", "5", "5x", "h5", "1d 2", "-3d", "0m"] {
            assert_eq!(parse_duration(bad), None, "{bad:?}");
        }
    }
}
