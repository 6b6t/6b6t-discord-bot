//! Tests against a real MariaDB: the class SQL on a fixture `player_stats`
//! database, the frozen snapshot, one vote per person and the vote/close rules.
//!
//! They run only when `POLLS_TEST_DATABASE_URL` points at a server where the
//! user may create databases (for example `mysql://poll:poll@127.0.0.1:3306`),
//! and are skipped otherwise. CI sets it to a MariaDB service container. Every
//! test creates and drops its own pair of databases.

#![allow(unknown_lints, clippy::assert_is_empty)]

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use chrono::{Duration, NaiveDate};
use futures::future::BoxFuture;
use poise::serenity_prelude as serenity;
use sqlx::{MySqlPool, mysql::MySqlPoolOptions};
use tokio::sync::Notify;

use super::{
    config::{PollConfig, contains_number, test_config},
    expr, render,
    service::{CloseOutcome, CreateRequest, PollService, Snapshot},
    store::{PollStore, VoteOutcome},
    transport::{EditRequest, EditResult, PollTransport},
};

/// 2026-10-06 12:00:00 UTC.
const CUTOFF_MS: i64 = 1_791_288_000_000;
const DAY_MS: i64 = 86_400_000;

/// Statements of the contract's final DDL (comments removed).
const ACTIVITY_DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS activity_crystal_fight (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT, started_at BIGINT NOT NULL, ended_at BIGINT NOT NULL, a_uuid CHAR(36) NOT NULL, b_uuid CHAR(36) NOT NULL, a_dmg INT UNSIGNED NOT NULL, b_dmg INT UNSIGNED NOT NULL, a_hits SMALLINT UNSIGNED NOT NULL, b_hits SMALLINT UNSIGNED NOT NULL, a_pops TINYINT UNSIGNED NOT NULL, b_pops TINYINT UNSIGNED NOT NULL, a_killed_b TINYINT UNSIGNED NOT NULL, b_killed_a TINYINT UNSIGNED NOT NULL, a_bot TINYINT UNSIGNED NOT NULL, b_bot TINYINT UNSIGNED NOT NULL, dim TINYINT NOT NULL, server VARCHAR(32) NOT NULL, PRIMARY KEY (id), UNIQUE KEY u_fight (a_uuid, b_uuid, started_at, server), KEY k_b (b_uuid, started_at), KEY k_t (started_at))",
    "CREATE TABLE IF NOT EXISTS activity_player (uuid CHAR(36) NOT NULL, is_bot TINYINT UNSIGNED NOT NULL, first_seen BIGINT NOT NULL, updated_at BIGINT NOT NULL, PRIMARY KEY (uuid), KEY k_bot (is_bot))",
    "CREATE TABLE IF NOT EXISTS activity_ip (uuid CHAR(36) NOT NULL, day DATE NOT NULL, ip_hash BINARY(12) NOT NULL, net_hash BINARY(12) NOT NULL, PRIMARY KEY (uuid, day, ip_hash), KEY k_ip (ip_hash, day), KEY k_day (day))",
    "CREATE TABLE IF NOT EXISTS activity_build_day (uuid CHAR(36) NOT NULL, day DATE NOT NULL, placed INT UNSIGNED NOT NULL, placed_obsidian INT UNSIGNED NOT NULL, mined INT UNSIGNED NOT NULL, PRIMARY KEY (uuid, day), KEY k_day (day))",
    "CREATE TABLE IF NOT EXISTS activity_build_material (uuid CHAR(36) NOT NULL, day DATE NOT NULL, material VARCHAR(48) NOT NULL, placed INT UNSIGNED NOT NULL, PRIMARY KEY (uuid, day, material), KEY k_day (day))",
    "CREATE TABLE IF NOT EXISTS activity_meta (name VARCHAR(64) NOT NULL, value BIGINT NOT NULL, updated_at BIGINT NOT NULL, PRIMARY KEY (name))",
];

const STATS_DDL: &[&str] = &[
    "CREATE TABLE player_info (uuid CHAR(36) NOT NULL PRIMARY KEY, name VARCHAR(16) NOT NULL, texture_hash VARCHAR(64) NULL, first_join BIGINT NOT NULL, last_join BIGINT NOT NULL)",
    "CREATE TABLE player_stats_per_day (uuid CHAR(36) NOT NULL, day DATE NOT NULL, type VARCHAR(50) NOT NULL, value BIGINT NOT NULL, PRIMARY KEY (uuid, day, type))",
];

const LINK_DDL: &str = "CREATE TABLE uuid_to_discord (uuid CHAR(36) NOT NULL PRIMARY KEY, discord_id VARCHAR(64) NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP, UNIQUE KEY unique_discord_id (discord_id))";

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Fixture {
    server: MySqlPool,
    link: MySqlPool,
    stats: MySqlPool,
    link_db: String,
    stats_db: String,
    base_url: String,
    /// Database users created by a test; dropped in `finish`.
    users: std::sync::Mutex<Vec<String>>,
}

/// A premium (version 4) uuid.
fn premium(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

/// An offline-mode (version 3) uuid, as cracked accounts have.
fn cracked(n: u32) -> String {
    format!("00000000-0000-3000-8000-{n:012}")
}

fn cutoff_day() -> NaiveDate {
    super::classes::cutoff_date(CUTOFF_MS)
}

fn days_before_cutoff(n: i64) -> NaiveDate {
    cutoff_day() - Duration::days(n)
}

async fn fixture(plugin_tables: bool) -> Option<Fixture> {
    let Ok(base) = std::env::var("POLLS_TEST_DATABASE_URL") else {
        eprintln!("POLLS_TEST_DATABASE_URL is not set; skipping the database test");
        return None;
    };
    let base = base.trim_end_matches('/').to_owned();
    let unique = format!(
        "polltest_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let server = MySqlPoolOptions::new()
        .max_connections(2)
        .connect(&base)
        .await
        .expect("connect to the test server");
    let link_db = format!("{unique}_link");
    let stats_db = format!("{unique}_stats");
    for database in [&link_db, &stats_db] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE `{database}` DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
        )))
        .execute(&server)
        .await
        .expect("create a test database");
    }
    let connect = |database: String| {
        let url = format!("{base}/{database}");
        async move {
            MySqlPoolOptions::new()
                .max_connections(4)
                .connect(&url)
                .await
                .expect("connect to a test database")
        }
    };
    let link = connect(link_db.clone()).await;
    let stats = connect(stats_db.clone()).await;
    for statement in STATS_DDL {
        sqlx::query(*statement).execute(&stats).await.unwrap();
    }
    if plugin_tables {
        for statement in ACTIVITY_DDL {
            sqlx::query(*statement).execute(&stats).await.unwrap();
        }
    }
    sqlx::query(LINK_DDL).execute(&link).await.unwrap();
    PollStore::new(link.clone()).migrate().await.unwrap();
    Some(Fixture {
        server,
        link,
        stats,
        link_db,
        stats_db,
        base_url: base,
        users: std::sync::Mutex::default(),
    })
}

impl Fixture {
    fn service(&self, config: PollConfig) -> PollService {
        PollService::new(self.link.clone(), self.stats.clone(), config)
    }

    /// A linked player. `first_join_days_ago` is relative to the cut-off.
    async fn player(&self, uuid: &str, name: &str, first_join_days_ago: i64) {
        sqlx::query(
            "INSERT INTO player_info (uuid, name, first_join, last_join) VALUES (?, ?, ?, ?)",
        )
        .bind(uuid)
        .bind(name)
        .bind(CUTOFF_MS - first_join_days_ago * DAY_MS)
        .bind(CUTOFF_MS - DAY_MS)
        .execute(&self.stats)
        .await
        .unwrap();
    }

    async fn link(&self, uuid: &str, discord_id: &str, linked_at: i64) {
        sqlx::query("INSERT INTO uuid_to_discord (uuid, discord_id, created_at) VALUES (?, ?, FROM_UNIXTIME(?))")
            .bind(uuid)
            .bind(discord_id)
            .bind(linked_at)
            .execute(&self.link)
            .await
            .unwrap();
    }

    /// Play time for one UTC day (`days_before` the cut-off day), in minutes.
    async fn play(&self, uuid: &str, days_before: i64, minutes: i64) {
        sqlx::query("INSERT INTO player_stats_per_day (uuid, day, type, value) VALUES (?, ?, 'play_time', ?) ON DUPLICATE KEY UPDATE value = VALUES(value)")
            .bind(uuid)
            .bind(days_before_cutoff(days_before))
            .bind(minutes * 1_200)
            .execute(&self.stats)
            .await
            .unwrap();
    }

    async fn mark_bot(&self, uuid: &str) {
        sqlx::query("INSERT INTO activity_player (uuid, is_bot, first_seen, updated_at) VALUES (?, 1, 0, 0) ON DUPLICATE KEY UPDATE is_bot = 1")
            .bind(uuid)
            .execute(&self.stats)
            .await
            .unwrap();
    }

    async fn meta(&self, name: &str, value: i64) {
        sqlx::query("INSERT INTO activity_meta (name, value, updated_at) VALUES (?, ?, 0) ON DUPLICATE KEY UPDATE value = VALUES(value)")
            .bind(name)
            .bind(value)
            .execute(&self.stats)
            .await
            .unwrap();
    }

    /// An IP observation on one of the UTC days before the cut-off day.
    async fn ip(&self, uuid: &str, days_before: i64, hash: u8) {
        sqlx::query(
            "INSERT IGNORE INTO activity_ip (uuid, day, ip_hash, net_hash) VALUES (?, ?, ?, ?)",
        )
        .bind(uuid)
        .bind(days_before_cutoff(days_before))
        .bind(vec![hash; 12])
        .bind(vec![0u8; 12])
        .execute(&self.stats)
        .await
        .unwrap();
    }

    /// A crystal fight starting `started_before_ms` before the cut-off and
    /// lasting `length_ms`. `a` must sort before `b`.
    async fn fight(&self, a: &str, b: &str, started_before_ms: i64, length_ms: i64, damage: u32) {
        assert!(a < b, "the plugin stores a_uuid < b_uuid");
        let started = CUTOFF_MS - started_before_ms;
        sqlx::query("INSERT INTO activity_crystal_fight (started_at, ended_at, a_uuid, b_uuid, a_dmg, b_dmg, a_hits, b_hits, a_pops, b_pops, a_killed_b, b_killed_a, a_bot, b_bot, dim, server) VALUES (?, ?, ?, ?, ?, 0, 3, 0, 0, 0, 0, 0, 0, 0, 2, 'worker-0')")
            .bind(started)
            .bind(started + length_ms)
            .bind(a)
            .bind(b)
            .bind(damage)
            .execute(&self.stats)
            .await
            .unwrap();
    }

    async fn build_day(
        &self,
        uuid: &str,
        days_before: i64,
        placed: u32,
        obsidian: u32,
        mined: u32,
    ) {
        sqlx::query("INSERT INTO activity_build_day (uuid, day, placed, placed_obsidian, mined) VALUES (?, ?, ?, ?, ?)")
            .bind(uuid)
            .bind(days_before_cutoff(days_before))
            .bind(placed)
            .bind(obsidian)
            .bind(mined)
            .execute(&self.stats)
            .await
            .unwrap();
    }

    async fn materials(&self, uuid: &str, days_before: i64, kinds: u32, each: u32) {
        for kind in 0..kinds {
            sqlx::query("INSERT INTO activity_build_material (uuid, day, material, placed) VALUES (?, ?, ?, ?)")
                .bind(uuid)
                .bind(days_before_cutoff(days_before))
                .bind(format!("BLOCK_{kind}"))
                .bind(each)
                .execute(&self.stats)
                .await
                .unwrap();
        }
    }

    async fn snapshot(&self, config: PollConfig, requires: &str, cutoff_ms: i64) -> Snapshot {
        let parsed = expr::parse(requires).unwrap();
        self.service(config)
            .build_snapshot(&parsed, requires, cutoff_ms)
            .await
            .unwrap()
    }

    async fn drop_table(&self, table: &str) {
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE `{table}`")))
            .execute(&self.stats)
            .await
            .unwrap();
    }

    /// A service whose stats connection is a database user that may `SELECT`
    /// only the listed stats tables (what a production grant looks like).
    /// `None` when the test server does not let the test user create users.
    async fn service_with_grants(
        &self,
        config: PollConfig,
        tables: &[&str],
    ) -> Option<PollService> {
        let user = format!(
            "pt_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let created = sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE USER '{user}'@'%' IDENTIFIED BY 'pw'"
        )))
        .execute(&self.server)
        .await;
        if let Err(error) = created {
            eprintln!("cannot create a database user ({error}); skipping the grants test");
            return None;
        }
        self.users.lock().unwrap().push(user.clone());
        for table in tables {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "GRANT SELECT ON `{}`.`{table}` TO '{user}'@'%'",
                self.stats_db
            )))
            .execute(&self.server)
            .await
            .unwrap();
        }
        let host = self
            .base_url
            .trim_start_matches("mysql://")
            .rsplit('@')
            .next()
            .unwrap()
            .to_owned();
        let restricted = MySqlPoolOptions::new()
            .max_connections(2)
            .connect(&format!("mysql://{user}:pw@{host}/{}", self.stats_db))
            .await
            .expect("connect as the restricted user");
        Some(PollService::new(self.link.clone(), restricted, config))
    }

    async fn finish(self) {
        self.link.close().await;
        self.stats.close().await;
        let users = self.users.lock().unwrap().clone();
        for user in users {
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP USER '{user}'@'%'")))
                .execute(&self.server)
                .await
                .unwrap();
        }
        for database in [&self.link_db, &self.stats_db] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE `{database}`")))
                .execute(&self.server)
                .await
                .unwrap();
        }
        self.server.close().await;
    }
}

fn discord_ids(snapshot: &Snapshot) -> BTreeSet<String> {
    snapshot
        .voters
        .iter()
        .map(|voter| voter.discord_id.clone())
        .collect()
}

fn ids(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn request(requires: &str) -> CreateRequest {
    CreateRequest {
        title: "Which one?".into(),
        options: vec!["Faster".into(), "Slower".into(), "Keep".into()],
        requires: requires.into(),
        duration_seconds: 3 * 86_400,
        channel_id: serenity::ChannelId::new(10),
        created_by: serenity::UserId::new(20),
    }
}

// ---- classes against the fixture database -------------------------------

#[tokio::test]
async fn veteran_uses_the_cutoff_and_the_cracked_guard() {
    let Some(db) = fixture(true).await else {
        return;
    };
    // The configured requirement is 100 days: 200 days is a veteran, 99 is not yet,
    // and exactly 100 is not either (strictly more than).
    db.player(&premium(1), "Old", 200).await;
    db.player(&premium(2), "Young", 99).await;
    db.player(&premium(3), "Edge", 100).await;
    // A premium uuid that kept the name of an old cracked account inherits its date.
    db.player(&premium(4), "Converted", 10).await;
    db.player(&cracked(4), "Converted", 400).await;
    // A premium uuid does NOT inherit the date of another premium account with its name.
    db.player(&premium(5), "Recycled", 10).await;
    db.player(&premium(6), "Recycled", 400).await;
    // Joined after the cut-off: first_join is in the future of this poll.
    sqlx::query(
        "INSERT INTO player_info (uuid, name, first_join, last_join) VALUES (?, 'Newcomer', ?, ?)",
    )
    .bind(premium(7))
    .bind(CUTOFF_MS + DAY_MS)
    .bind(CUTOFF_MS + DAY_MS)
    .execute(&db.stats)
    .await
    .unwrap();
    for (n, discord) in [
        (1, "101"),
        (2, "102"),
        (3, "103"),
        (4, "104"),
        (5, "105"),
        (7, "107"),
    ] {
        db.link(&premium(n), discord, 1_000 + i64::from(n)).await;
    }
    let snapshot = db.snapshot(test_config(), "veteran", CUTOFF_MS).await;
    assert_eq!(discord_ids(&snapshot), ids(&["101", "104"]));
    // A longer requirement keeps only the converted account (its cracked row is 400 days old).
    let strict = db
        .snapshot(test_config(), "veteran(days=300)", CUTOFF_MS)
        .await;
    assert_eq!(discord_ids(&strict), ids(&["104"]));
    let strictest = db
        .snapshot(test_config(), "veteran(days=401)", CUTOFF_MS)
        .await;
    assert!(strictest.voters.is_empty());
    db.finish().await;
}

#[tokio::test]
async fn activity_tiers_count_whole_utc_days_before_the_cutoff_day() {
    let Some(db) = fixture(true).await else {
        return;
    };
    // Test config: active = 3 days of 5+ minutes in 6; very_active = 4 days of 60+
    // minutes in 6; overall_active = 2 days of 5+ minutes in 10.
    // 10: four days of 15 minutes.
    // 11: two days, plus big rows ON the cut-off day and after it.
    // 12: six days of three hours.
    // 13: six days of 45 minutes.
    // 14: four days of only 4 minutes.
    for (n, discord) in [
        (10, "110"),
        (11, "111"),
        (12, "112"),
        (13, "113"),
        (14, "114"),
    ] {
        db.player(&premium(n), &format!("P{n}"), 30).await;
        db.link(&premium(n), discord, 1_000).await;
    }
    for day in 1..=4 {
        db.play(&premium(10), day, 15).await;
        db.play(&premium(14), day, 4).await;
    }
    for day in 1..=2 {
        db.play(&premium(11), day, 300).await;
    }
    db.play(&premium(11), 0, 600).await; // the cut-off day itself
    db.play(&premium(11), -1, 600).await; // the day after the cut-off
    for day in 1..=6 {
        db.play(&premium(12), day, 180).await;
        db.play(&premium(13), day, 45).await;
    }
    let cfg = test_config;
    let active = db.snapshot(cfg(), "active", CUTOFF_MS).await;
    assert_eq!(discord_ids(&active), ids(&["110", "112", "113"]));
    let very = db.snapshot(cfg(), "very_active", CUTOFF_MS).await;
    assert_eq!(discord_ids(&very), ids(&["112"]));
    // overall_active: player 11 has exactly two days before the cut-off day;
    // its cut-off-day rows add nothing.
    let overall = db.snapshot(cfg(), "overall_active", CUTOFF_MS).await;
    assert_eq!(discord_ids(&overall), ids(&["110", "111", "112", "113"]));
    // Moving the cut-off one day later brings the cut-off-day row into the
    // window for the same data: 11 then has three days and becomes active.
    let later = db.snapshot(cfg(), "active", CUTOFF_MS + DAY_MS).await;
    assert!(discord_ids(&later).contains("111"));
    // Combined expressions use set algebra.
    let both = db
        .snapshot(cfg(), "active AND NOT very_active", CUTOFF_MS)
        .await;
    assert_eq!(discord_ids(&both), ids(&["110", "113"]));
    let either = db
        .snapshot(
            cfg(),
            "very_active OR (overall_active AND NOT active)",
            CUTOFF_MS,
        )
        .await;
    assert_eq!(discord_ids(&either), ids(&["111", "112"]));
    db.finish().await;
}

#[tokio::test]
async fn crystal_fights_must_end_before_the_cutoff_and_involve_real_opponents() {
    let Some(db) = fixture(true).await else {
        return;
    };
    // Player 20 (low uuid) fights 21, 22, 23 twice each, one fight 3 days ago and one 1 day ago.
    for n in [20, 21, 22, 23, 24] {
        db.player(&premium(n), &format!("P{n}"), 100).await;
    }
    db.link(&premium(20), "120", 1_000).await;
    db.link(&premium(24), "124", 1_000).await;
    for opponent in [21, 22, 23] {
        db.fight(&premium(20), &premium(opponent), 3 * DAY_MS, 20_000, 100)
            .await;
        db.fight(&premium(20), &premium(opponent), DAY_MS, 20_000, 100)
            .await;
    }
    // Player 24 has the same fights, but they all end after the cut-off.
    for opponent in [21, 22, 23] {
        db.fight(&premium(opponent), &premium(24), 1_000, 20_000, 100)
            .await;
        db.fight(&premium(opponent), &premium(24), 2_000, 20_000, 100)
            .await;
    }
    db.meta("recording_since.crystal", CUTOFF_MS - 30 * DAY_MS)
        .await;
    let snapshot = db.snapshot(test_config(), "crystal_pvper", CUTOFF_MS).await;
    assert_eq!(discord_ids(&snapshot), ids(&["120"]));

    // A fight in progress at the cut-off (starts before, ends at the cut-off) never counts.
    let extra_fight_only = {
        db.fight(&premium(20), &premium(21), 10_000, 10_000, 100)
            .await;
        db.snapshot(test_config(), "crystal_pvper(fights=7)", CUTOFF_MS)
            .await
    };
    assert!(
        discord_ids(&extra_fight_only).is_empty(),
        "6 counted fights, the 7th ends at the cut-off"
    );
    let with_cutoff_later = db
        .snapshot(test_config(), "crystal_pvper(fights=7)", CUTOFF_MS + 60_000)
        .await;
    assert_eq!(
        discord_ids(&with_cutoff_later),
        ids(&["120"]),
        "once the cut-off passes its end it counts"
    );

    // Marking the opponents as bots removes their fights.
    for opponent in [21, 22] {
        db.mark_bot(&premium(opponent)).await;
    }
    let after_marking = db.snapshot(test_config(), "crystal_pvper", CUTOFF_MS).await;
    assert!(
        discord_ids(&after_marking).is_empty(),
        "late bot marks still apply"
    );
    db.finish().await;
}

#[tokio::test]
async fn builder_follows_the_contract_and_ignores_newer_days() {
    let Some(db) = fixture(true).await else {
        return;
    };
    for n in [30, 31, 32] {
        db.player(&premium(n), &format!("B{n}"), 100).await;
        db.link(&premium(n), &format!("1{n}"), 1_000).await;
    }
    // 30: six days of 1,000 blocks, 30 materials, little mining: a builder.
    // 31: the same but with a lot of obsidian: the farm pattern.
    // 32: qualifies only with rows on and after the cut-off day.
    for day in 1..=6 {
        db.build_day(&premium(30), day, 1_000, 0, 20).await;
        db.build_day(&premium(31), day, 1_000, 2_000, 20).await;
    }
    db.materials(&premium(30), 2, 30, 20).await;
    db.materials(&premium(31), 2, 30, 20).await;
    for day in [3, 2, 1] {
        db.build_day(&premium(32), day, 100, 0, 5).await;
    }
    for day in [0, -1, -2, -3, -4] {
        db.build_day(&premium(32), day, 2_000, 0, 5).await;
    }
    db.materials(&premium(32), 0, 40, 50).await;
    db.meta("recording_since.build", CUTOFF_MS - 30 * DAY_MS)
        .await;
    let snapshot = db.snapshot(test_config(), "builder", CUTOFF_MS).await;
    assert_eq!(discord_ids(&snapshot), ids(&["130"]));
    db.finish().await;
}

#[tokio::test]
async fn bots_and_unlinked_players_never_count() {
    let Some(db) = fixture(true).await else {
        return;
    };
    for n in [40, 41, 42] {
        db.player(&premium(n), &format!("V{n}"), 300).await;
    }
    db.link(&premium(40), "140", 1_000).await;
    db.link(&premium(41), "141", 1_000).await;
    // 42 is a veteran but not linked.
    db.mark_bot(&premium(41)).await;
    let snapshot = db.snapshot(test_config(), "veteran", CUTOFF_MS).await;
    assert_eq!(discord_ids(&snapshot), ids(&["140"]));
    // NOT is evaluated among linked humans only: the bot and the unlinked player stay out.
    let not_active = db.snapshot(test_config(), "NOT active", CUTOFF_MS).await;
    assert_eq!(discord_ids(&not_active), ids(&["140"]));
    db.finish().await;
}

#[tokio::test]
async fn one_vote_per_person_cluster() {
    let Some(db) = fixture(true).await else {
        return;
    };
    for n in [50, 51, 52, 53, 54, 55] {
        db.player(&premium(n), &format!("V{n}"), 300).await;
    }
    // 50 and 51 are alts that share an address. 51's Discord linked first.
    db.link(&premium(50), "150", 2_000).await;
    db.link(&premium(51), "151", 1_000).await;
    db.ip(&premium(50), 2, 7).await;
    db.ip(&premium(51), 3, 7).await;
    // 52 and 53 are linked only through an unlinked alt (the plugin saw it, the link table did not).
    db.player(&premium(60), "Hidden", 300).await;
    db.link(&premium(52), "152", 3_000).await;
    db.link(&premium(53), "153", 4_000).await;
    db.ip(&premium(52), 2, 8).await;
    db.ip(&premium(60), 2, 8).await;
    db.ip(&premium(60), 3, 9).await;
    db.ip(&premium(53), 3, 9).await;
    // 54 and 55 share only an address that five accounts use (an internet cafe): different people.
    db.link(&premium(54), "154", 5_000).await;
    db.link(&premium(55), "155", 6_000).await;
    for n in [54, 55, 70, 71, 72] {
        if n >= 70 {
            db.player(&premium(n), &format!("C{n}"), 5).await;
        }
        db.ip(&premium(n), 2, 10).await;
    }
    let snapshot = db.snapshot(test_config(), "veteran", CUTOFF_MS).await;
    assert_eq!(discord_ids(&snapshot), ids(&["151", "152", "154", "155"]));
    let keys: BTreeSet<_> = snapshot
        .voters
        .iter()
        .map(|voter| voter.person_key.clone())
        .collect();
    assert_eq!(keys.len(), snapshot.voters.len(), "person keys are unique");
    db.finish().await;
}

// ---- the frozen snapshot -------------------------------------------------

#[tokio::test]
async fn newer_data_never_changes_a_snapshot_or_a_vote() {
    let Some(db) = fixture(true).await else {
        return;
    };
    // 60 is eligible now (4 active days). 61 is not (2 days) and later farms the data.
    for n in [60, 61] {
        db.player(&premium(n), &format!("P{n}"), 30).await;
    }
    db.link(&premium(60), "160", 1_000).await;
    db.link(&premium(61), "161", 1_000).await;
    for day in 1..=4 {
        db.play(&premium(60), day, 30).await;
    }
    for day in 1..=2 {
        db.play(&premium(61), day, 30).await;
    }
    let service = db.service(test_config());
    let before = match service
        .prepare_at(&request("active"), CUTOFF_MS)
        .await
        .unwrap()
    {
        Ok(prepared) => prepared,
        Err(message) => panic!("rejected: {message}"),
    };
    let poll_id = service
        .store_for_tests()
        .create_poll(&before.new_poll, &before.voters)
        .await
        .unwrap();

    // After the poll started: player 61 plays every day (on the cut-off day
    // and after, the only days that are new), and a new veteran links.
    for day in [0, -1, -2, -3, -4, -5] {
        db.play(&premium(61), day, 240).await;
    }
    db.player(&premium(62), "Late", 400).await;
    db.link(&premium(62), "162", 9_000).await;

    // Evaluating again with the same cut-off sees nothing new for 61 ...
    let parsed = expr::parse("active").unwrap();
    let again = service
        .build_snapshot(&parsed, "active", CUTOFF_MS)
        .await
        .unwrap();
    assert_eq!(discord_ids(&again), ids(&["160"]));
    // ... and the vote check never looks at class data at all: only the stored snapshot.
    let reply = service.process_vote(poll_id, "161", 0).await.unwrap();
    assert_eq!(
        reply,
        render::ineligible_reason("regular recent play on 6b6t")
    );
    let reply = service.process_vote(poll_id, "162", 0).await.unwrap();
    assert!(reply.starts_with("You need a linked Minecraft account"));
    let reply = service.process_vote(poll_id, "160", 1).await.unwrap();
    assert!(reply.contains("**Slower**") && reply.contains("is counted"));

    // The snapshot rows are exactly what was frozen.
    let stored: Vec<String> = sqlx::query_scalar(
        "SELECT discord_id FROM poll_eligible WHERE poll_id = ? ORDER BY discord_id",
    )
    .bind(poll_id)
    .fetch_all(&db.link)
    .await
    .unwrap();
    assert_eq!(stored, vec!["160".to_owned()]);
    db.finish().await;
}

#[tokio::test]
async fn data_after_the_cutoff_cannot_make_anyone_eligible() {
    let Some(db) = fixture(true).await else {
        return;
    };
    db.player(&premium(80), "Farmer", 30).await;
    db.link(&premium(80), "180", 1_000).await;
    // Nothing before the cut-off day; plenty on it and after it.
    for day in [0, -1, -2, -3, -4, -5] {
        db.play(&premium(80), day, 240).await;
    }
    for opponent in 81..=84 {
        db.player(&premium(opponent), &format!("O{opponent}"), 100)
            .await;
        db.fight(&premium(80), &premium(opponent), -DAY_MS, 20_000, 100)
            .await; // after the cut-off
        db.fight(&premium(80), &premium(opponent), -2 * DAY_MS, 20_000, 100)
            .await;
    }
    for day in [0, -1, -2, -3] {
        db.build_day(&premium(80), day, 5_000, 0, 10).await;
    }
    db.materials(&premium(80), -1, 40, 50).await;
    db.meta("recording_since.crystal", CUTOFF_MS - 30 * DAY_MS)
        .await;
    db.meta("recording_since.build", CUTOFF_MS - 30 * DAY_MS)
        .await;
    for requires in [
        "active",
        "very_active",
        "overall_active",
        "crystal_pvper",
        "builder",
    ] {
        let snapshot = db.snapshot(test_config(), requires, CUTOFF_MS).await;
        assert!(
            snapshot.voters.is_empty(),
            "{requires} must not see data after the cut-off"
        );
    }
    db.finish().await;
}

// ---- votes, denials and closing -----------------------------------------

async fn open_poll(db: &Fixture, service: &PollService) -> u64 {
    db.player(&premium(90), "A", 400).await;
    db.player(&premium(91), "B", 400).await;
    db.player(&premium(92), "C", 10).await;
    db.link(&premium(90), "190", 1_000).await;
    db.link(&premium(91), "191", 1_000).await;
    db.link(&premium(92), "192", 1_000).await;
    let prepared = match service
        .prepare_at(&request("veteran"), CUTOFF_MS)
        .await
        .unwrap()
    {
        Ok(prepared) => prepared,
        Err(message) => panic!("rejected: {message}"),
    };
    service
        .store_for_tests()
        .create_poll(&prepared.new_poll, &prepared.voters)
        .await
        .unwrap()
}

#[tokio::test]
async fn voting_changing_and_refusing() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = open_poll(&db, &service).await;
    let store = service.store_for_tests();

    let first = service.process_vote(poll_id, "190", 0).await.unwrap();
    assert_eq!(first, render::vote_reply("Faster", true, false));
    let same = service.process_vote(poll_id, "190", 0).await.unwrap();
    assert_eq!(same, render::vote_reply("Faster", false, false));
    let changed = service.process_vote(poll_id, "190", 2).await.unwrap();
    assert_eq!(changed, render::vote_reply("Keep", false, true));
    service.process_vote(poll_id, "191", 1).await.unwrap();

    // One row per voter: changing the vote did not add a second one.
    let votes = store.votes(poll_id).await.unwrap();
    assert_eq!(votes.len(), 2);
    assert!(votes.contains(&("190".to_owned(), 2)));

    // Not eligible: a private reason without numbers, counted but not stored.
    let denied = service.process_vote(poll_id, "192", 0).await.unwrap();
    assert!(denied.starts_with("You need a linked Minecraft account with a long history on 6b6t"));
    assert!(!contains_number(&denied));
    let stranger = service.process_vote(poll_id, "999", 0).await.unwrap();
    assert_eq!(
        stranger, denied,
        "an unlinked stranger gets the same answer"
    );
    assert_eq!(store.votes(poll_id).await.unwrap().len(), 2);
    assert_eq!(
        store.denials(poll_id).await.unwrap(),
        vec![("not_eligible".to_owned(), 2)]
    );
    // An option index outside the poll is refused.
    assert!(matches!(
        store.cast_vote(poll_id, "190", 3, 3).await.unwrap(),
        VoteOutcome::InvalidOption
    ));
    assert_eq!(
        service.process_vote(poll_id, "190", 0).await.unwrap(),
        render::vote_reply("Faster", false, true)
    );
    assert_eq!(
        service.process_vote(424_242, "190", 0).await.unwrap(),
        render::MISSING_REPLY
    );
    db.finish().await;
}

#[tokio::test]
async fn nothing_counts_after_the_end_time_or_a_close() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = open_poll(&db, &service).await;
    let store = service.store_for_tests();
    service.process_vote(poll_id, "190", 0).await.unwrap();

    // The end time passes: the vote is refused even before the worker has closed the poll.
    sqlx::query("UPDATE polls SET ends_at = UNIX_TIMESTAMP() - 5 WHERE poll_id = ?")
        .bind(poll_id)
        .execute(&db.link)
        .await
        .unwrap();
    assert_eq!(
        service.process_vote(poll_id, "191", 1).await.unwrap(),
        render::CLOSED_REPLY
    );
    assert_eq!(store.due().await.unwrap(), vec![poll_id]);
    // Closing is claimed exactly once.
    assert!(store.claim_close(poll_id, false).await.unwrap());
    assert!(!store.claim_close(poll_id, false).await.unwrap());
    assert!(!store.claim_close(poll_id, true).await.unwrap());
    assert_eq!(
        service.process_vote(poll_id, "190", 2).await.unwrap(),
        render::CLOSED_REPLY
    );
    assert_eq!(
        store.votes(poll_id).await.unwrap(),
        vec![("190".to_owned(), 0)]
    );
    assert_eq!(store.unfinalized().await.unwrap(), vec![poll_id]);
    store
        .finalize(poll_id, "{\"counts\":[1,0,0]}")
        .await
        .unwrap();
    assert!(store.unfinalized().await.unwrap().is_empty());
    let text = service.results(poll_id).await.unwrap().unwrap();
    assert!(text.contains("- Faster: 1 (100%)") && text.contains("(closed)"));
    assert!(
        !text.contains("eligible"),
        "turnout stays hidden by default"
    );
    db.finish().await;
}

#[tokio::test]
async fn staff_can_close_early_and_turnout_is_a_toggle() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let config = PollConfig {
        show_turnout: true,
        ..test_config()
    };
    let service = db.service(config);
    let poll_id = open_poll(&db, &service).await;
    let store = service.store_for_tests();
    service.process_vote(poll_id, "190", 1).await.unwrap();
    service.process_vote(poll_id, "192", 1).await.unwrap(); // refused
    let open_results = service.results(poll_id).await.unwrap().unwrap();
    assert!(open_results.contains("Votes counted: 1 of 2 eligible"));
    assert!(open_results.contains("not_eligible: 1"));
    assert!(
        store.claim_close(poll_id, true).await.unwrap(),
        "close ignores the end time"
    );
    db.finish().await;
}

#[tokio::test]
async fn purging_old_polls_keeps_totals_and_removes_voter_rows() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = open_poll(&db, &service).await;
    let store = service.store_for_tests();
    service.process_vote(poll_id, "190", 0).await.unwrap();
    assert!(store.claim_close(poll_id, true).await.unwrap());
    store
        .finalize(poll_id, "{\"counts\":[1,0,0]}")
        .await
        .unwrap();
    assert_eq!(store.purge_voter_rows(365).await.unwrap(), 0, "too recent");
    sqlx::query("UPDATE polls SET closed_at = UNIX_TIMESTAMP() - 400 * 86400 WHERE poll_id = ?")
        .bind(poll_id)
        .execute(&db.link)
        .await
        .unwrap();
    assert_eq!(
        store.purge_voter_rows(365).await.unwrap(),
        3,
        "one vote and two snapshot rows"
    );
    assert!(store.votes(poll_id).await.unwrap().is_empty());
    let text = service.results(poll_id).await.unwrap().unwrap();
    assert!(text.contains("- Faster: 1"), "the stored result survives");
    db.finish().await;
}

#[tokio::test]
async fn migrations_are_idempotent() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let store = PollStore::new(db.link.clone());
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poll_migrations")
        .fetch_one(&db.link)
        .await
        .unwrap();
    assert_eq!(applied, 1);
    for table in ["polls", "poll_eligible", "poll_votes", "poll_denials"] {
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?")
            .bind(table)
            .fetch_one(&db.link)
            .await
            .unwrap();
        assert_eq!(exists, 1, "{table}");
    }
    db.finish().await;
}

// ---- creating a poll: validation and data coverage ----------------------

#[tokio::test]
async fn create_requests_are_validated_before_anything_is_stored() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    db.player(&premium(1), "Old", 400).await;
    db.link(&premium(1), "101", 1_000).await;
    let reject = |message: Result<super::service::Prepared, String>| {
        message.expect_err("should be rejected")
    };

    let bad_expression = reject(
        service
            .prepare_at(&request("veteran AND"), CUTOFF_MS)
            .await
            .unwrap(),
    );
    assert!(bad_expression.contains("not valid"), "{bad_expression}");
    let unknown = reject(
        service
            .prepare_at(&request("whale"), CUTOFF_MS)
            .await
            .unwrap(),
    );
    assert!(unknown.contains("unknown class"));
    let mut one_option = request("veteran");
    one_option.options.truncate(1);
    assert!(
        reject(service.prepare_at(&one_option, CUTOFF_MS).await.unwrap()).contains("2-5 options")
    );
    let mut short = request("veteran");
    short.duration_seconds = 30;
    assert!(reject(service.prepare_at(&short, CUTOFF_MS).await.unwrap()).contains("duration"));
    let mut long = request("veteran");
    long.duration_seconds = 400 * 86_400;
    assert!(reject(service.prepare_at(&long, CUTOFF_MS).await.unwrap()).contains("duration"));
    let nobody = reject(
        service
            .prepare_at(&request("very_active"), CUTOFF_MS)
            .await
            .unwrap(),
    );
    assert!(nobody.contains("Nobody is eligible"));
    let polls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM polls")
        .fetch_one(&db.link)
        .await
        .unwrap();
    assert_eq!(polls, 0);

    let ok = service
        .prepare_at(&request("veteran"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ok.voters.len(), 1);
    assert_eq!(ok.new_poll.requires_expr, "veteran");
    assert!(ok.new_poll.rule_json.contains("\"call\":\"veteran\""));
    assert!(
        ok.new_poll.rule_json.contains("\"days\":100"),
        "the thresholds are recorded internally"
    );
    db.finish().await;
}

#[tokio::test]
async fn crystal_and_builder_polls_wait_for_a_fully_recorded_window() {
    let Some(db) = fixture(true).await else {
        return;
    };
    db.player(&premium(1), "Old", 400).await;
    db.link(&premium(1), "101", 1_000).await;
    let service = db.service(test_config());
    let refused = service
        .prepare_at(&request("crystal_pvper"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap_err();
    assert!(refused.contains("has not started recording"), "{refused}");
    // Four days of recording is not enough for a five-day window.
    db.meta("recording_since.crystal", CUTOFF_MS - 4 * DAY_MS)
        .await;
    let refused = service
        .prepare_at(&request("crystal_pvper"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        refused.contains("does not cover the whole window"),
        "{refused}"
    );
    // Staff can accept partial data in config; the poll then carries a warning.
    let config = PollConfig {
        allow_partial_data: true,
        ..test_config()
    };
    let service = db.service(config);
    let refused = service
        .prepare_at(&request("crystal_pvper"), CUTOFF_MS)
        .await
        .unwrap();
    // Nobody fought, so it is rejected for another reason, but not for coverage.
    assert!(refused.unwrap_err().contains("Nobody is eligible"));
    let with_warning = service
        .prepare_at(&request("veteran OR crystal_pvper"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap();
    assert!(
        with_warning
            .warnings
            .iter()
            .any(|warning| warning.contains("recording started"))
    );
    db.finish().await;
}

fn voter_ids(prepared: &super::service::Prepared) -> BTreeSet<String> {
    prepared
        .voters
        .iter()
        .map(|voter| voter.discord_id.clone())
        .collect()
}

// ---- review fix 1: identity evidence of the cut-off day ------------------

#[tokio::test]
async fn ip_evidence_of_the_cutoff_day_merges_alts() {
    let Some(db) = fixture(true).await else {
        return;
    };
    // Test config: the IP window is 20 days before the cut-off day up to and
    // including the cut-off day; an address on more than 3 accounts links nobody.
    for n in (100..=109).chain(120..=122).chain(130..=133).chain([110]) {
        db.player(&premium(n), &format!("V{n}"), 300).await;
    }
    // 100 and 101 shared an address only on the cut-off day. 101 linked first.
    db.link(&premium(100), "d100", 2_000).await;
    db.link(&premium(101), "d101", 1_000).await;
    db.ip(&premium(100), 0, 1).await;
    db.ip(&premium(101), 0, 1).await;
    // 102 and 103 are joined through an unlinked alt (110), both links on the cut-off day.
    db.link(&premium(102), "d102", 3_000).await;
    db.link(&premium(103), "d103", 4_000).await;
    db.ip(&premium(102), 0, 2).await;
    db.ip(&premium(110), 0, 2).await;
    db.ip(&premium(110), 0, 3).await;
    db.ip(&premium(103), 0, 3).await;
    // 104 and 105 share an address only on the day AFTER the cut-off day: not evidence yet.
    db.link(&premium(104), "d104", 1_000).await;
    db.link(&premium(105), "d105", 1_000).await;
    db.ip(&premium(104), -1, 4).await;
    db.ip(&premium(105), -1, 4).await;
    // 106 and 107 share an address on the first day of the window: still evidence.
    db.link(&premium(106), "d106", 5_000).await;
    db.link(&premium(107), "d107", 6_000).await;
    db.ip(&premium(106), 20, 5).await;
    db.ip(&premium(107), 20, 5).await;
    // 108 and 109 share one on the day before the window: too old.
    db.link(&premium(108), "d108", 1_000).await;
    db.link(&premium(109), "d109", 1_000).await;
    db.ip(&premium(108), 21, 6).await;
    db.ip(&premium(109), 21, 6).await;
    // Three accounts on one address (the limit) are one person; four are a crowd. Cut-off day only.
    for (n, linked) in [(120, 9_000), (121, 8_000), (122, 7_000)] {
        db.link(&premium(n), &format!("d{n}"), linked).await;
        db.ip(&premium(n), 0, 7).await;
    }
    for n in 130..=133 {
        db.link(&premium(n), &format!("d{n}"), 1_000).await;
        db.ip(&premium(n), 0, 8).await;
    }
    let snapshot = db.snapshot(test_config(), "veteran", CUTOFF_MS).await;
    assert_eq!(
        discord_ids(&snapshot),
        ids(&[
            "d101", "d102", "d104", "d105", "d106", "d108", "d109", "d122", "d130", "d131", "d132",
            "d133"
        ])
    );
    db.finish().await;
}

#[tokio::test]
async fn cutoff_day_ip_evidence_decides_who_is_a_real_crystal_opponent() {
    let Some(db) = fixture(true).await else {
        return;
    };
    for n in [20, 21, 22, 23, 70, 71] {
        db.player(&premium(n), &format!("P{n}"), 100).await;
    }
    db.link(&premium(20), "120", 1_000).await;
    for opponent in [21, 22, 23] {
        db.fight(&premium(20), &premium(opponent), 3 * DAY_MS, 20_000, 100)
            .await;
        db.fight(&premium(20), &premium(opponent), DAY_MS, 20_000, 100)
            .await;
    }
    db.meta("recording_since.crystal", CUTOFF_MS - 30 * DAY_MS)
        .await;
    // Six fights against three opponents: eligible (four fights, two opponents needed).
    let plain = db.snapshot(test_config(), "crystal_pvper", CUTOFF_MS).await;
    assert_eq!(discord_ids(&plain), ids(&["120"]));

    // 21 and 22 turn out to share an address with 20, seen only on the cut-off day:
    // those were the same person, so only 23's two fights count.
    for alt in [21, 22] {
        db.ip(&premium(20), 0, u8::try_from(alt).unwrap()).await;
        db.ip(&premium(alt), 0, u8::try_from(alt).unwrap()).await;
    }
    let alts = db.snapshot(test_config(), "crystal_pvper", CUTOFF_MS).await;
    assert!(
        discord_ids(&alts).is_empty(),
        "fights against the same person must not count"
    );

    // Four accounts on 21's address (more than the limit of three): a shared
    // address, links nobody, so its fights count again (4 fights, 2 opponents).
    db.ip(&premium(70), 0, 21).await;
    db.ip(&premium(71), 0, 21).await;
    let crowded = db.snapshot(test_config(), "crystal_pvper", CUTOFF_MS).await;
    assert_eq!(discord_ids(&crowded), ids(&["120"]));
    db.finish().await;
}

// ---- review fix 2: bot flags and identity without the other tables -------

async fn two_alts_and_a_bot(db: &Fixture) {
    for n in [40, 41, 42, 43] {
        db.player(&premium(n), &format!("V{n}"), 300).await;
    }
    db.link(&premium(40), "140", 1_000).await;
    db.link(&premium(41), "141", 1_000).await; // marked as a bot below
    db.link(&premium(42), "142", 2_000).await;
    db.link(&premium(43), "143", 1_500).await; // links first: wins over 42
    db.mark_bot(&premium(41)).await;
    db.ip(&premium(42), 0, 7).await;
    db.ip(&premium(43), 0, 7).await;
}

#[tokio::test]
async fn known_bot_marks_and_ip_evidence_survive_a_missing_unrelated_table() {
    let Some(db) = fixture(true).await else {
        return;
    };
    two_alts_and_a_bot(&db).await;
    db.drop_table("activity_build_material").await;
    let service = db.service(test_config());
    for requires in ["veteran", "veteran OR active"] {
        let prepared = service
            .prepare_at(&request(requires), CUTOFF_MS)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(voter_ids(&prepared), ids(&["140", "143"]), "{requires}");
    }
    // A poll that needs the missing table is refused, naming it.
    db.meta("recording_since.build", CUTOFF_MS - 30 * DAY_MS)
        .await;
    let refused = service
        .prepare_at(&request("veteran OR builder"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        refused.contains("activity_build_material") && refused.contains("`builder`"),
        "{refused}"
    );
    // Crystal polls do not need it.
    db.meta("recording_since.crystal", CUTOFF_MS - 30 * DAY_MS)
        .await;
    let crystal = service
        .prepare_at(&request("veteran OR crystal_pvper"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(voter_ids(&crystal), ids(&["140", "143"]));
    db.finish().await;
}

#[tokio::test]
async fn a_table_the_bot_may_not_select_counts_as_missing() {
    let Some(db) = fixture(true).await else {
        return;
    };
    two_alts_and_a_bot(&db).await;
    let all_but_material = [
        "player_info",
        "player_stats_per_day",
        "activity_crystal_fight",
        "activity_player",
        "activity_ip",
        "activity_build_day",
        "activity_meta",
    ];
    let Some(service) = db
        .service_with_grants(test_config(), &all_but_material)
        .await
    else {
        db.finish().await;
        return;
    };
    let prepared = service
        .prepare_at(&request("veteran"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(voter_ids(&prepared), ids(&["140", "143"]));
    db.meta("recording_since.build", CUTOFF_MS - 30 * DAY_MS)
        .await;
    let refused = service
        .prepare_at(&request("builder"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap_err();
    assert!(refused.contains("activity_build_material"), "{refused}");

    // Without the grant on the bot flags nothing runs.
    let without_bots: Vec<&str> = all_but_material
        .iter()
        .copied()
        .filter(|table| *table != "activity_player")
        .collect();
    let blind = db
        .service_with_grants(test_config(), &without_bots)
        .await
        .unwrap();
    let refused = blind
        .prepare_at(&request("veteran"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        refused.contains("activity_player") && refused.contains("no poll can run"),
        "{refused}"
    );
    db.finish().await;
}

#[tokio::test]
async fn polls_are_refused_when_the_bot_flags_cannot_be_read() {
    for plugin_tables in [false, true] {
        let Some(db) = fixture(plugin_tables).await else {
            return;
        };
        if plugin_tables {
            db.drop_table("activity_player").await;
        }
        db.player(&premium(1), "Old", 400).await;
        db.link(&premium(1), "101", 1_000).await;
        let service = db.service(test_config());
        for requires in ["veteran", "active", "crystal_pvper"] {
            let refused = service
                .prepare_at(&request(requires), CUTOFF_MS)
                .await
                .unwrap()
                .unwrap_err();
            assert!(
                refused.contains("activity_player") && refused.contains("no poll can run"),
                "{requires}: {refused}"
            );
        }
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM polls")
            .fetch_one(&db.link)
            .await
            .unwrap();
        assert_eq!(stored, 0);
        db.finish().await;
    }
}

#[tokio::test]
async fn a_missing_ip_or_coverage_table_warns_but_keeps_bot_exclusion() {
    let Some(db) = fixture(true).await else {
        return;
    };
    two_alts_and_a_bot(&db).await;
    db.drop_table("activity_ip").await;
    db.drop_table("activity_meta").await;
    let service = db.service(test_config());
    let prepared = service
        .prepare_at(&request("veteran"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap();
    // The bot (141) is out; with no IP table the alts cannot be merged (and the
    // staff warning says so); 140, 142 and 143 stay.
    assert_eq!(voter_ids(&prepared), ids(&["140", "142", "143"]));
    let warnings = prepared.warnings.join("\n");
    assert!(warnings.contains("activity_ip") && warnings.contains("alt detection is off"));
    assert!(warnings.contains("activity_meta"));
    let refused = service
        .prepare_at(&request("crystal_pvper"), CUTOFF_MS)
        .await
        .unwrap()
        .unwrap_err();
    assert!(refused.contains("activity_meta"), "{refused}");
    db.finish().await;
}

// ---- review fix 3: a coherent creation boundary --------------------------

#[tokio::test]
async fn links_at_or_after_the_cutoff_second_never_enter() {
    let Some(db) = fixture(true).await else {
        return;
    };
    for n in 1..=6 {
        db.player(&premium(n), &format!("V{n}"), 300).await;
    }
    let cutoff_seconds = CUTOFF_MS / 1000;
    db.link(&premium(1), "101", cutoff_seconds - 1).await;
    db.link(&premium(2), "102", cutoff_seconds).await;
    db.link(&premium(3), "103", cutoff_seconds + 1).await;
    db.link(&premium(4), "104", cutoff_seconds + 3_600).await;
    // A link row without a timestamp is of unknown age: it is not dropped for that.
    sqlx::query(
        "INSERT INTO uuid_to_discord (uuid, discord_id, created_at) VALUES (?, '105', NULL)",
    )
    .bind(premium(5))
    .execute(&db.link)
    .await
    .unwrap();
    let service = db.service(test_config());
    let prepared = service
        .prepare_at(&request("veteran"), CUTOFF_MS + 500)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        voter_ids(&prepared),
        ids(&["101", "105"]),
        "the same second as the cut-off cannot be shown to be earlier"
    );
    // A poll started an hour later does see the links made in between.
    let later = service
        .prepare_at(&request("veteran"), CUTOFF_MS + 3_700_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(voter_ids(&later), ids(&["101", "102", "103", "104", "105"]));
    db.finish().await;
}

#[tokio::test]
async fn a_link_made_while_the_poll_is_being_created_does_not_enter() {
    let Some(db) = fixture(true).await else {
        return;
    };
    db.player(&premium(1), "Old", 300).await;
    db.player(&premium(2), "Slow", 300).await;
    db.link(&premium(1), "101", 1_000).await;
    let service = db.service(test_config());
    let wanted = request("veteran");
    let validated = service.validate(&wanted).unwrap();
    let sealed = service.seal_with(&|| CUTOFF_MS).await.unwrap();
    // The cut-off is fixed and the class queries are running: a second
    // account links now. The link is not read again, whatever its timestamp says.
    db.link(&premium(2), "102", 1_000).await;
    let prepared = service
        .prepare_sealed(&wanted, &validated, sealed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(voter_ids(&prepared), ids(&["101"]));
    db.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cutoff_is_fixed_after_the_snapshot_reads() {
    let Some(db) = fixture(true).await else {
        return;
    };
    for n in [1, 2, 3] {
        db.player(&premium(n), &format!("V{n}"), 300).await;
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let handle = tokio::runtime::Handle::current();
    let link_pool = db.link.clone();
    // The clock is read once before the snapshot reads and once after them.
    // Each call also links one more account, from inside the clock.
    let clock = {
        let calls = Arc::clone(&calls);
        move || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            let late = match call {
                0 => Some((premium(1), "101")),
                1 => Some((premium(2), "102")),
                _ => None,
            };
            if let Some((uuid, discord)) = late {
                let pool = link_pool.clone();
                tokio::task::block_in_place(|| {
                    handle.block_on(async move {
                        sqlx::query("INSERT INTO uuid_to_discord (uuid, discord_id, created_at) VALUES (?, ?, FROM_UNIXTIME(1000))")
                            .bind(uuid)
                            .bind(discord)
                            .execute(&pool)
                            .await
                            .unwrap();
                    });
                });
            }
            CUTOFF_MS
        }
    };
    let service = db.service(test_config());
    let prepared = service
        .prepare_with(&request("veteran"), &clock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // 101 linked before anything was read: in. 102 linked after the reads but
    // before the cut-off was fixed: not read, so not in. The cut-off itself is
    // the second reading.
    assert_eq!(voter_ids(&prepared), ids(&["101"]));
    assert_eq!(prepared.new_poll.cutoff_ms, CUTOFF_MS);
    db.finish().await;
}

#[tokio::test]
async fn reads_are_repeated_when_utc_midnight_passes_in_the_middle() {
    let Some(db) = fixture(true).await else {
        return;
    };
    for n in [1, 2] {
        db.player(&premium(n), &format!("V{n}"), 300).await;
    }
    db.link(&premium(1), "101", 1_000).await;
    db.link(&premium(2), "102", 2_000).await;
    // The two accounts share an address seen only on the day that starts at midnight.
    db.ip(&premium(1), -1, 9).await;
    db.ip(&premium(2), -1, 9).await;
    let midnight = CUTOFF_MS + 12 * 3_600_000;
    let times = [
        midnight - 100,
        midnight + 100,
        midnight + 200,
        midnight + 300,
    ];
    let calls = AtomicUsize::new(0);
    let clock = move || times[calls.fetch_add(1, Ordering::SeqCst).min(3)];
    let prepared = service_prepare(&db, &clock).await;
    // The first attempt would have read up to the day before: the retry reads the new day.
    assert_eq!(voter_ids(&prepared), ids(&["101"]), "one person");
    assert_eq!(prepared.new_poll.cutoff_ms, midnight + 300);
    db.finish().await;
}

async fn service_prepare(db: &Fixture, clock: &super::service::Clock) -> super::service::Prepared {
    db.service(test_config())
        .prepare_with(&request("veteran"), clock)
        .await
        .unwrap()
        .unwrap()
}

// ---- review fix 4: redraw, close and finalize are serialized -------------

/// An edit parked in the fake: which kind, and the two signals.
type Hold = (bool, Arc<Notify>, Arc<Notify>);

/// One landed edit: `(closed, counts)`.
type Landed = (bool, Option<Vec<u32>>);

/// A Discord that records what the poll message ends up showing. One edit can
/// be parked inside it, to hold a worker at the moment a close arrives.
#[derive(Default)]
struct FakeTransport {
    /// `(closed, counts)` of every edit, in the order the edits landed.
    landed: std::sync::Mutex<Vec<Landed>>,
    membership_checks: AtomicUsize,
    hold: std::sync::Mutex<Option<Hold>>,
}

impl FakeTransport {
    /// Parks the next edit whose `closed` flag matches. Returns `(arrived,
    /// release)`: the edit signals `arrived` and then waits for `release`.
    fn hold_next(&self, closed: bool) -> (Arc<Notify>, Arc<Notify>) {
        let arrived = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        *self.hold.lock().unwrap() = Some((closed, arrived.clone(), release.clone()));
        (arrived, release)
    }

    fn landed(&self) -> Vec<Landed> {
        self.landed.lock().unwrap().clone()
    }

    fn checks(&self) -> usize {
        self.membership_checks.load(Ordering::SeqCst)
    }
}

impl PollTransport for FakeTransport {
    fn edit_message(&self, request: EditRequest) -> BoxFuture<'_, Result<EditResult>> {
        Box::pin(async move {
            let held = {
                let mut slot = self.hold.lock().unwrap();
                if slot
                    .as_ref()
                    .is_some_and(|(closed, ..)| *closed == request.closed)
                {
                    slot.take()
                } else {
                    None
                }
            };
            if let Some((_, arrived, release)) = held {
                arrived.notify_one();
                release.notified().await;
            }
            self.landed
                .lock()
                .unwrap()
                .push((request.closed, request.counts));
            Ok(EditResult::Done)
        })
    }

    fn member_gone(&self, _user_id: u64) -> BoxFuture<'_, bool> {
        self.membership_checks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { false })
    }
}

/// Fails a test that would otherwise hang when a schedule does not unfold as planned.
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(15), future)
        .await
        .expect("timed out: the schedule did not unfold as planned")
}

/// An open poll with a posted message, two votes in and a redraw due. The
/// service is started (`ready`), as the worker does nothing before that.
async fn poll_with_a_redraw_due(db: &Fixture, service: &PollService) -> u64 {
    service.ready().await.unwrap();
    let poll_id = open_poll(db, service).await;
    service
        .store_for_tests()
        .attach_message(poll_id, "555")
        .await
        .unwrap();
    service.process_vote(poll_id, "190", 0).await.unwrap();
    service.process_vote(poll_id, "191", 1).await.unwrap();
    sqlx::query("UPDATE polls SET last_render_ms = 0 WHERE poll_id = ?")
        .bind(poll_id)
        .execute(&db.link)
        .await
        .unwrap();
    poll_id
}

async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

#[tokio::test]
async fn a_pending_redraw_cannot_overwrite_the_closed_message() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = poll_with_a_redraw_due(&db, &service).await;
    let transport = Arc::new(FakeTransport::default());
    let (arrived, release) = transport.hold_next(false);

    // The worker loaded the open poll and is about to edit its message ...
    let worker = tokio::spawn({
        let (service, transport) = (service.clone(), Arc::clone(&transport));
        async move { service.worker_with(&*transport).await }
    });
    within(arrived.notified()).await;
    // ... when staff close the poll.
    let close = tokio::spawn({
        let (service, transport) = (service.clone(), Arc::clone(&transport));
        async move { service.close_with(&*transport, poll_id).await }
    });
    settle().await;
    assert!(
        transport.landed().is_empty(),
        "the final edit must wait for the redraw that is in flight"
    );
    release.notify_one();
    within(worker).await.unwrap();
    let CloseOutcome::Closed { text } = within(close).await.unwrap().unwrap() else {
        panic!("the poll was open, so close must succeed");
    };

    // The open redraw landed first; the closed message is what stays.
    assert_eq!(
        transport.landed(),
        vec![(false, Some(vec![1, 1, 0])), (true, Some(vec![1, 1, 0]))]
    );
    assert!(text.contains("- Faster: 1"));
    // Nothing is left to redraw or finalize: another tick edits nothing.
    service.worker_with(&*transport).await;
    assert_eq!(transport.landed().len(), 2);
    db.finish().await;
}

#[tokio::test]
async fn a_redraw_that_starts_after_the_close_does_nothing() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = poll_with_a_redraw_due(&db, &service).await;
    let transport = FakeTransport::default();
    service.close_with(&transport, poll_id).await.unwrap();
    // A redraw claimed from a list that was loaded before the close.
    sqlx::query("UPDATE polls SET dirty = 1 WHERE poll_id = ?")
        .bind(poll_id)
        .execute(&db.link)
        .await
        .unwrap();
    service.refresh_one(&transport, poll_id).await.unwrap();
    assert_eq!(transport.landed(), vec![(true, Some(vec![1, 1, 0]))]);
    db.finish().await;
}

#[tokio::test]
async fn the_worker_and_a_manual_close_do_not_both_finalize() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = poll_with_a_redraw_due(&db, &service).await;
    let transport = Arc::new(FakeTransport::default());
    let (arrived, release) = transport.hold_next(true);

    // Staff close the poll: it is claimed, and the closed message is being edited ...
    let close = tokio::spawn({
        let (service, transport) = (service.clone(), Arc::clone(&transport));
        async move { service.close_with(&*transport, poll_id).await }
    });
    within(arrived.notified()).await;
    // ... when the worker ticks, sees a closed poll that is not finalized, and joins in.
    let worker = tokio::spawn({
        let (service, transport) = (service.clone(), Arc::clone(&transport));
        async move { service.worker_with(&*transport).await }
    });
    settle().await;
    assert_eq!(
        transport.checks(),
        2,
        "the worker must not run its own membership check meanwhile"
    );
    assert!(transport.landed().is_empty());
    release.notify_one();
    let CloseOutcome::Closed { text } = within(close).await.unwrap().unwrap() else {
        panic!("the poll was open, so close must succeed");
    };
    within(worker).await.unwrap();

    assert_eq!(
        transport.landed(),
        vec![(true, Some(vec![1, 1, 0]))],
        "one closed edit"
    );
    assert_eq!(transport.checks(), 2, "voters were checked once, not twice");
    assert!(text.contains("- Faster: 1") && text.contains("- Slower: 1"));
    let stored: String = sqlx::query_scalar("SELECT result_json FROM polls WHERE poll_id = ?")
        .bind(poll_id)
        .fetch_one(&db.link)
        .await
        .unwrap();
    assert!(stored.contains("\"counts\":[1,1,0]"), "{stored}");
    db.finish().await;
}

#[tokio::test]
async fn a_close_that_loses_the_finalize_to_the_worker_shows_the_stored_result() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = poll_with_a_redraw_due(&db, &service).await;
    let transport = Arc::new(FakeTransport::default());
    // Hold the poll's lock, the way a finalize in flight does.
    let in_flight = service.lock_poll(poll_id).await;
    let close = tokio::spawn({
        let (service, transport) = (service.clone(), Arc::clone(&transport));
        async move { service.close_with(&*transport, poll_id).await }
    });
    settle().await;
    assert!(
        !close.is_finished(),
        "close claimed the poll and waits to finalize"
    );
    // The other finalizer finishes: result stored, message edited.
    assert!(
        service
            .store_for_tests()
            .finalize(
                poll_id,
                "{\"counts\":[1,1,0],\"total\":2,\"left_server\":0}"
            )
            .await
            .unwrap()
    );
    drop(in_flight);
    let CloseOutcome::Closed { text } = within(close).await.unwrap().unwrap() else {
        panic!("the poll was open, so close must succeed");
    };
    assert!(
        text.contains("- Faster: 1") && text.contains("(closed)") && !text.contains("will retry"),
        "{text}"
    );
    assert!(
        transport.landed().is_empty() && transport.checks() == 0,
        "close must not edit or check anybody a second time"
    );
    db.finish().await;
}

#[tokio::test]
async fn a_closed_poll_is_not_finalized_twice_across_processes() {
    let Some(db) = fixture(true).await else {
        return;
    };
    let service = db.service(test_config());
    let poll_id = open_poll(&db, &service).await;
    let store = service.store_for_tests();
    assert!(store.claim_close(poll_id, true).await.unwrap());
    assert!(
        store
            .finalize(poll_id, "{\"counts\":[1,0,0]}")
            .await
            .unwrap()
    );
    assert!(
        !store
            .finalize(poll_id, "{\"counts\":[0,1,0]}")
            .await
            .unwrap()
    );
    let stored: String = sqlx::query_scalar("SELECT result_json FROM polls WHERE poll_id = ?")
        .bind(poll_id)
        .fetch_one(&db.link)
        .await
        .unwrap();
    assert_eq!(stored, "{\"counts\":[1,0,0]}");
    db.finish().await;
}
