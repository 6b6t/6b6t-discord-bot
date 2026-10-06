//! Poll tables in the bot's link database: migrations, snapshot, votes, close.

use anyhow::{Context as _, Result};
use sqlx::{MySql, MySqlPool, Row as _, Transaction};

use super::identity::Voter;

/// Migrations in order. Each is a file of idempotent statements.
const MIGRATIONS: &[(i32, &str)] = &[(
    1,
    include_str!("../../migrations/0001_eligibility_polls.sql"),
)];

/// Splits a migration file into statements, dropping `--` comment lines.
fn statements(sql: &str) -> Vec<String> {
    let without_comments: String = sql
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    without_comments
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .map(str::to_owned)
        .collect()
}

pub async fn migrate(pool: &MySqlPool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS poll_migrations (version INT NOT NULL PRIMARY KEY, applied_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
    )
    .execute(pool)
    .await
    .context("failed to create the poll migration table")?;
    for (version, sql) in MIGRATIONS {
        let applied: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM poll_migrations WHERE version = ?")
                .bind(version)
                .fetch_one(pool)
                .await?;
        if applied > 0 {
            continue;
        }
        for statement in statements(sql) {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(pool)
                .await
                .with_context(|| format!("poll migration {version} failed"))?;
        }
        sqlx::query("INSERT IGNORE INTO poll_migrations (version) VALUES (?)")
            .bind(version)
            .execute(pool)
            .await?;
        tracing::info!(version, "applied poll migration");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Open,
    Closed,
    Cancelled,
}

impl Status {
    pub fn parse(value: &str) -> Self {
        match value {
            "open" => Self::Open,
            "closed" => Self::Closed,
            _ => Self::Cancelled,
        }
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct PollRow {
    pub poll_id: u64,
    pub channel_id: String,
    pub message_id: Option<String>,
    pub title: String,
    pub options_json: String,
    pub requires_expr: String,
    pub cutoff_ms: i64,
    pub window_days: i32,
    pub ends_at: i64,
    pub status: String,
    pub eligible_count: i32,
    pub finalized: i8,
    pub result_json: Option<String>,
}

impl PollRow {
    pub fn status(&self) -> Status {
        Status::parse(&self.status)
    }

    pub fn options(&self) -> Result<Vec<String>> {
        serde_json::from_str(&self.options_json).context("stored poll options are invalid")
    }
}

const POLL_COLUMNS: &str = "poll_id, channel_id, message_id, title, options_json, requires_expr, cutoff_ms, window_days, ends_at, status, eligible_count, finalized, result_json";

/// What a new poll stores, besides its voters.
#[derive(Clone, Debug)]
pub struct NewPoll {
    pub channel_id: String,
    pub title: String,
    pub options: Vec<String>,
    pub requires_expr: String,
    pub rule_json: String,
    pub cutoff_ms: i64,
    pub window_days: i32,
    pub duration_seconds: i64,
    pub created_by: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoteOutcome {
    /// First vote, or the same option again.
    Counted {
        first: bool,
        changed: bool,
    },
    /// Not in the frozen snapshot.
    NotEligible,
    /// Closed, cancelled or past its end time.
    Closed,
    Missing,
    InvalidOption,
}

#[derive(Clone)]
pub struct PollStore {
    pool: MySqlPool,
}

impl PollStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<()> {
        migrate(&self.pool).await
    }

    /// Stores the poll and its frozen snapshot in one transaction.
    pub async fn create_poll(&self, new: &NewPoll, voters: &[Voter]) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let options_json = serde_json::to_string(&new.options)?;
        let result = sqlx::query(
            "INSERT INTO polls (channel_id, title, options_json, requires_expr, rule_json, cutoff_ms, window_days, starts_at, ends_at, status, eligible_count, created_by) VALUES (?, ?, ?, ?, ?, ?, ?, UNIX_TIMESTAMP(), UNIX_TIMESTAMP() + ?, 'open', ?, ?)",
        )
        .bind(&new.channel_id)
        .bind(&new.title)
        .bind(options_json)
        .bind(&new.requires_expr)
        .bind(&new.rule_json)
        .bind(new.cutoff_ms)
        .bind(new.window_days)
        .bind(new.duration_seconds)
        .bind(i32::try_from(voters.len()).unwrap_or(i32::MAX))
        .bind(&new.created_by)
        .execute(&mut *tx)
        .await
        .context("failed to insert the poll")?;
        let poll_id = result.last_insert_id();
        for chunk in voters.chunks(200) {
            let mut builder = sqlx::QueryBuilder::<MySql>::new(
                "INSERT INTO poll_eligible (poll_id, discord_id, uuid, person_key) ",
            );
            builder.push_values(chunk, |mut row, voter| {
                row.push_bind(poll_id)
                    .push_bind(&voter.discord_id)
                    .push_bind(&voter.uuid)
                    .push_bind(&voter.person_key);
            });
            builder
                .build()
                .execute(&mut *tx)
                .await
                .context("failed to store the poll snapshot")?;
        }
        tx.commit().await?;
        Ok(poll_id)
    }

    pub async fn attach_message(&self, poll_id: u64, message_id: &str) -> Result<()> {
        sqlx::query("UPDATE polls SET message_id = ?, last_render_ms = UNIX_TIMESTAMP() * 1000 WHERE poll_id = ?")
            .bind(message_id)
            .bind(poll_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Cancels a poll whose message could not be posted.
    pub async fn cancel(&self, poll_id: u64) -> Result<()> {
        sqlx::query("UPDATE polls SET status = 'cancelled', finalized = 1, closed_at = UNIX_TIMESTAMP() WHERE poll_id = ? AND status = 'open'")
            .bind(poll_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Cancels an open poll whose message was deleted. Returns the rows changed.
    pub async fn cancel_by_message(&self, channel_id: &str, message_id: &str) -> Result<u64> {
        let result = sqlx::query("UPDATE polls SET status = 'cancelled', finalized = 1, closed_at = UNIX_TIMESTAMP() WHERE channel_id = ? AND message_id = ? AND status = 'open'")
            .bind(channel_id)
            .bind(message_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn poll(&self, poll_id: u64) -> Result<Option<PollRow>> {
        sqlx::query_as::<_, PollRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {POLL_COLUMNS} FROM polls WHERE poll_id = ?"
        )))
        .bind(poll_id)
        .fetch_optional(&self.pool)
        .await
        .context("failed to load the poll")
    }

    /// Casts or changes a vote against the frozen snapshot only. The poll row
    /// is locked, so a vote can never land after the poll closed.
    pub async fn cast_vote(
        &self,
        poll_id: u64,
        discord_id: &str,
        option: usize,
        option_count: usize,
    ) -> Result<VoteOutcome> {
        let option_idx = match u8::try_from(option) {
            Ok(value) if option < option_count => value,
            _ => return Ok(VoteOutcome::InvalidOption),
        };
        let mut tx = self.pool.begin().await?;
        let state = sqlx::query(
            "SELECT status = 'open' AND ends_at > UNIX_TIMESTAMP() AS accepting FROM polls WHERE poll_id = ? FOR UPDATE",
        )
        .bind(poll_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(state) = state else {
            return Ok(VoteOutcome::Missing);
        };
        let accepting: i64 = state.try_get("accepting")?;
        if accepting == 0 {
            return Ok(VoteOutcome::Closed);
        }
        let eligible: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM poll_eligible WHERE poll_id = ? AND discord_id = ?",
        )
        .bind(poll_id)
        .bind(discord_id)
        .fetch_one(&mut *tx)
        .await?;
        if eligible == 0 {
            return Ok(VoteOutcome::NotEligible);
        }
        let previous: Option<u8> = sqlx::query_scalar(
            "SELECT option_idx FROM poll_votes WHERE poll_id = ? AND voter_discord_id = ?",
        )
        .bind(poll_id)
        .bind(discord_id)
        .fetch_optional(&mut *tx)
        .await?;
        let changed = previous.is_some_and(|previous| previous != option_idx);
        sqlx::query(
            "INSERT INTO poll_votes (poll_id, voter_discord_id, option_idx, voted_at) VALUES (?, ?, ?, UNIX_TIMESTAMP()) ON DUPLICATE KEY UPDATE option_idx = VALUES(option_idx), changed_at = IF(option_idx <> VALUES(option_idx), UNIX_TIMESTAMP(), changed_at)",
        )
        .bind(poll_id)
        .bind(discord_id)
        .bind(option_idx)
        .execute(&mut *tx)
        .await?;
        if previous.is_none() || changed {
            sqlx::query("UPDATE polls SET dirty = 1 WHERE poll_id = ?")
                .bind(poll_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(VoteOutcome::Counted {
            first: previous.is_none(),
            changed,
        })
    }

    pub async fn record_denial(&self, poll_id: u64, reason_code: &str) -> Result<()> {
        sqlx::query("INSERT INTO poll_denials (poll_id, reason_code, n) VALUES (?, ?, 1) ON DUPLICATE KEY UPDATE n = n + 1")
            .bind(poll_id)
            .bind(reason_code)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn denials(&self, poll_id: u64) -> Result<Vec<(String, u32)>> {
        let rows = sqlx::query(
            "SELECT reason_code, n FROM poll_denials WHERE poll_id = ? ORDER BY reason_code",
        )
        .bind(poll_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| Ok((row.try_get("reason_code")?, row.try_get("n")?)))
            .collect()
    }

    /// Every stored vote that belongs to a snapshot member, as
    /// `(discord_id, option_idx)`. Votes from outside the snapshot cannot
    /// exist, but the join keeps the tally honest even if they did.
    pub async fn votes(&self, poll_id: u64) -> Result<Vec<(String, u8)>> {
        let rows = sqlx::query(
            "SELECT v.voter_discord_id, v.option_idx FROM poll_votes v JOIN poll_eligible e ON e.poll_id = v.poll_id AND e.discord_id = v.voter_discord_id WHERE v.poll_id = ?",
        )
        .bind(poll_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| Ok((row.try_get("voter_discord_id")?, row.try_get("option_idx")?)))
            .collect()
    }

    /// Open polls whose end time has passed.
    pub async fn due(&self) -> Result<Vec<u64>> {
        sqlx::query_scalar(
            "SELECT poll_id FROM polls WHERE status = 'open' AND ends_at <= UNIX_TIMESTAMP() ORDER BY poll_id LIMIT 20",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to load due polls")
    }

    /// Closes an open poll. Returns false when somebody else closed it first.
    /// With `force` the end time is ignored (`/poll close`).
    pub async fn claim_close(&self, poll_id: u64, force: bool) -> Result<bool> {
        let statement = if force {
            "UPDATE polls SET status = 'closed', closed_at = UNIX_TIMESTAMP(), dirty = 1 WHERE poll_id = ? AND status = 'open'"
        } else {
            "UPDATE polls SET status = 'closed', closed_at = UNIX_TIMESTAMP(), dirty = 1 WHERE poll_id = ? AND status = 'open' AND ends_at <= UNIX_TIMESTAMP()"
        };
        let result = sqlx::query(statement)
            .bind(poll_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Closed polls that still need their result computed and message edited.
    pub async fn unfinalized(&self) -> Result<Vec<u64>> {
        sqlx::query_scalar(
            "SELECT poll_id FROM polls WHERE status = 'closed' AND finalized = 0 ORDER BY poll_id LIMIT 10",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to load unfinalized polls")
    }

    pub async fn finalize(&self, poll_id: u64, result_json: &str) -> Result<()> {
        sqlx::query("UPDATE polls SET finalized = 1, dirty = 0, result_json = ? WHERE poll_id = ? AND status = 'closed'")
            .bind(result_json)
            .bind(poll_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Open polls with a changed tally that were not edited in the last
    /// `min_gap_ms`. The caller claims each with [`Self::claim_render`].
    pub async fn dirty(&self, min_gap_ms: i64) -> Result<Vec<u64>> {
        sqlx::query_scalar(
            "SELECT poll_id FROM polls WHERE status = 'open' AND dirty = 1 AND message_id IS NOT NULL AND last_render_ms <= (UNIX_TIMESTAMP(NOW(3)) * 1000) - ? ORDER BY poll_id LIMIT 20",
        )
        .bind(min_gap_ms)
        .fetch_all(&self.pool)
        .await
        .context("failed to load polls to refresh")
    }

    pub async fn claim_render(&self, poll_id: u64) -> Result<bool> {
        let result = sqlx::query("UPDATE polls SET dirty = 0, last_render_ms = UNIX_TIMESTAMP(NOW(3)) * 1000 WHERE poll_id = ? AND dirty = 1")
            .bind(poll_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_dirty(&self, poll_id: u64) -> Result<()> {
        sqlx::query("UPDATE polls SET dirty = 1 WHERE poll_id = ?")
            .bind(poll_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Open polls whose message was never attached (the bot stopped between
    /// storing the snapshot and posting). Cancelled by the worker.
    pub async fn orphaned(&self, older_than_seconds: i64) -> Result<Vec<u64>> {
        sqlx::query_scalar(
            "SELECT poll_id FROM polls WHERE status = 'open' AND message_id IS NULL AND starts_at <= UNIX_TIMESTAMP() - ?",
        )
        .bind(older_than_seconds)
        .fetch_all(&self.pool)
        .await
        .context("failed to load orphaned polls")
    }

    /// Deletes voter rows (snapshot, votes, denial counters stay as totals) of
    /// polls closed longer than `retention_days` ago.
    pub async fn purge_voter_rows(&self, retention_days: i64) -> Result<u64> {
        let mut tx: Transaction<'_, MySql> = self.pool.begin().await?;
        let expired: Vec<u64> = sqlx::query_scalar(
            "SELECT poll_id FROM polls WHERE status IN ('closed', 'cancelled') AND finalized = 1 AND closed_at IS NOT NULL AND closed_at < UNIX_TIMESTAMP() - ? * 86400",
        )
        .bind(retention_days)
        .fetch_all(&mut *tx)
        .await?;
        let mut deleted = 0;
        for poll_id in expired {
            deleted += sqlx::query("DELETE FROM poll_votes WHERE poll_id = ?")
                .bind(poll_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            deleted += sqlx::query("DELETE FROM poll_eligible WHERE poll_id = ?")
                .bind(poll_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        }
        tx.commit().await?;
        Ok(deleted)
    }
}

#[cfg(test)]
#[allow(unknown_lints, clippy::assert_is_empty)]
mod tests {
    use super::*;

    #[test]
    fn migration_files_split_into_idempotent_statements() {
        for (_, sql) in MIGRATIONS {
            let parsed = statements(sql);
            assert!(!parsed.is_empty());
            for statement in parsed {
                assert!(
                    statement.starts_with("CREATE TABLE IF NOT EXISTS"),
                    "migration statements must be idempotent: {statement}"
                );
                assert!(!statement.contains("--"));
            }
        }
        let first = statements(MIGRATIONS[0].1);
        assert_eq!(first.len(), 4);
    }

    #[test]
    fn status_parses_unknown_values_as_cancelled() {
        assert_eq!(Status::parse("open"), Status::Open);
        assert_eq!(Status::parse("closed"), Status::Closed);
        assert_eq!(Status::parse("whatever"), Status::Cancelled);
    }
}
