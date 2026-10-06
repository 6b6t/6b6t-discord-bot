//! Eligibility-gated button polls.
//!
//! A poll declares a `requires` expression over player classes (`veteran`,
//! `active`, `crystal_pvper`, ...). When staff create it, the bot freezes the
//! eligible set from data strictly before that moment, stores it in
//! `poll_eligible`, and posts a button poll. Votes are checked against that
//! snapshot only. Nothing about eligibility is visible to players: thresholds
//! live in [`config::PollConfig`], the poll text names the window and says in
//! plain words who can vote, and an ineligible click gets a short private reply.

pub mod classes;
pub mod config;
pub mod expr;
pub mod identity;
pub mod render;
pub mod service;
pub mod store;
pub mod transport;

#[cfg(test)]
mod integration_tests;

use std::env;

use anyhow::{Context as _, Result, bail};

use crate::config::{DatabaseConfig, env_bool, optional_env};

pub use service::PollService;

/// Poll settings read from the environment.
#[derive(Clone, Debug)]
pub struct PollSettings {
    pub config: config::PollConfig,
    /// Dedicated (ideally read-only) credentials for the `player_stats`
    /// database. `None` means: use the bot's existing stats connection.
    pub stats_database: Option<DatabaseConfig>,
}

/// Optional overrides for the connection that reads `player_stats`.
#[derive(Clone, Debug, Default)]
pub struct StatsOverride {
    pub host: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
}

impl PollSettings {
    /// Reads `POLLS_*`. Returns `None` unless `POLLS_ENABLED=true`, so
    /// deploying the code changes nothing until the feature is switched on.
    ///
    /// | Variable | Purpose |
    /// | --- | --- |
    /// | `POLLS_ENABLED` | `true` to enable polls (default `false`) |
    /// | `POLLS_CONFIG` | inline JSON, see [`config::PollConfig`]; one of the two is required |
    /// | `POLLS_CONFIG_FILE` | path to the same JSON (not together with `POLLS_CONFIG`) |
    /// | `POLLS_STATS_DB_USER`, `POLLS_STATS_DB_PASS` | read-only credentials for `player_stats` (together) |
    /// | `POLLS_STATS_DB_HOST`, `POLLS_STATS_DB_PORT`, `POLLS_STATS_DB_NAME` | override the host, port or database name of that connection |
    pub fn load(database: Option<&DatabaseConfig>) -> Result<Option<Self>> {
        if !env_bool("POLLS_ENABLED", false) {
            return Ok(None);
        }
        let inline = optional_env("POLLS_CONFIG");
        let file = optional_env("POLLS_CONFIG_FILE");
        let json = match (inline, file) {
            (Some(_), Some(_)) => bail!("set only one of POLLS_CONFIG and POLLS_CONFIG_FILE"),
            (Some(inline), None) => Some(inline),
            (None, Some(path)) => Some(
                std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read POLLS_CONFIG_FILE {path}"))?,
            ),
            (None, None) => None,
        };
        let overrides = StatsOverride {
            host: optional_env("POLLS_STATS_DB_HOST"),
            port: optional_env("POLLS_STATS_DB_PORT"),
            user: optional_env("POLLS_STATS_DB_USER"),
            password: env::var("POLLS_STATS_DB_PASS")
                .ok()
                .filter(|value| !value.is_empty()),
            database: optional_env("POLLS_STATS_DB_NAME"),
        };
        Self::from_parts(json.as_deref(), &overrides, database).map(Some)
    }

    pub fn from_parts(
        json: Option<&str>,
        overrides: &StatsOverride,
        database: Option<&DatabaseConfig>,
    ) -> Result<Self> {
        let Some(base) = database else {
            bail!("polls need MySQL (MYSQL_DB_HOST and friends) to be configured");
        };
        let Some(json) = json else {
            bail!(
                "POLLS_CONFIG or POLLS_CONFIG_FILE is required: the thresholds are deliberately not part of the (public) code"
            );
        };
        let config = config::PollConfig::from_json(json)?;
        if overrides.user.is_some() != overrides.password.is_some() {
            bail!("POLLS_STATS_DB_USER and POLLS_STATS_DB_PASS must be set together");
        }
        let any_override = overrides.host.is_some()
            || overrides.port.is_some()
            || overrides.user.is_some()
            || overrides.database.is_some();
        let stats_database = any_override
            .then(|| -> Result<DatabaseConfig> {
                let mut stats = base.clone();
                if let Some(host) = &overrides.host {
                    stats.host.clone_from(host);
                }
                if let Some(port) = &overrides.port {
                    stats.port = port
                        .parse()
                        .context("POLLS_STATS_DB_PORT must be a port number")?;
                }
                if let (Some(user), Some(password)) = (&overrides.user, &overrides.password) {
                    stats.user.clone_from(user);
                    stats.password.clone_from(password);
                }
                if let Some(name) = &overrides.database {
                    stats.stats_database.clone_from(name);
                }
                Ok(stats)
            })
            .transpose()?;
        Ok(Self {
            config,
            stats_database,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{"identity": {"window_days": 20, "hub_limit": 3}}"#;

    fn base() -> DatabaseConfig {
        DatabaseConfig {
            host: "db".into(),
            port: 3306,
            user: "bot".into(),
            password: "secret".into(),
            link_database: "player_link".into(),
            stats_database: "player_stats".into(),
        }
    }

    #[test]
    fn defaults_reuse_the_existing_stats_connection() {
        let settings =
            PollSettings::from_parts(Some(CONFIG), &StatsOverride::default(), Some(&base()))
                .unwrap();
        assert!(settings.stats_database.is_none());
        assert!(!settings.config.show_turnout);
    }

    #[test]
    fn a_dedicated_read_only_user_overrides_only_what_is_given() {
        let overrides = StatsOverride {
            user: Some("poll_ro".into()),
            password: Some("pw".into()),
            database: Some("player_stats_replica".into()),
            ..StatsOverride::default()
        };
        let settings = PollSettings::from_parts(Some(CONFIG), &overrides, Some(&base())).unwrap();
        let stats = settings.stats_database.unwrap();
        assert_eq!(stats.user, "poll_ro");
        assert_eq!(stats.password, "pw");
        assert_eq!(stats.host, "db");
        assert_eq!(stats.stats_database, "player_stats_replica");
    }

    #[test]
    fn invalid_settings_are_errors() {
        let none = StatsOverride::default();
        let user_only = StatsOverride {
            user: Some("poll_ro".into()),
            ..StatsOverride::default()
        };
        assert!(PollSettings::from_parts(Some(CONFIG), &user_only, Some(&base())).is_err());
        assert!(PollSettings::from_parts(Some("{\"nope\":1}"), &none, Some(&base())).is_err());
        assert!(PollSettings::from_parts(Some(CONFIG), &none, None).is_err());
        let bad_port = StatsOverride {
            port: Some("abc".into()),
            ..StatsOverride::default()
        };
        assert!(PollSettings::from_parts(Some(CONFIG), &bad_port, Some(&base())).is_err());
    }

    #[test]
    fn thresholds_must_come_from_config() {
        let error =
            PollSettings::from_parts(None, &StatsOverride::default(), Some(&base())).unwrap_err();
        assert!(error.to_string().contains("POLLS_CONFIG"));
    }
}
