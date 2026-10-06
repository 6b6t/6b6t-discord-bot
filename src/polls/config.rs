//! Poll configuration: every threshold and window lives in deployment config
//! and nowhere else.
//!
//! This repository is public, so no threshold is built into the code: the
//! class settings have no defaults, and a class that is not configured cannot
//! be used in a poll. The values come from the `POLLS_CONFIG` (inline JSON) or
//! `POLLS_CONFIG_FILE` (path to a JSON file) environment variable and are never
//! shown to players. Unknown fields are rejected so a typo cannot silently
//! disable a rule.

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use super::expr::Class;

/// Minecraft ticks per minute, the unit of `player_stats_per_day` play time.
pub const TICKS_PER_MINUTE: i64 = 1_200;

/// The validated poll configuration.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct PollConfig {
    /// Show "N of M eligible voted" in results. Off by default: the eligible
    /// total would hint at the thresholds.
    pub show_turnout: bool,
    /// Show running counts on the open poll message. When off, counts appear
    /// only after the poll closes.
    pub show_live_counts: bool,
    /// At close, drop votes from people who have left the server.
    pub drop_departed_voters: bool,
    /// Let staff create a crystal or builder poll before the plugin has
    /// recorded the whole window (default: refuse).
    pub allow_partial_data: bool,
    pub min_duration_minutes: i64,
    pub max_duration_days: i64,
    /// Closed polls keep their snapshot and votes this long, then the voter
    /// rows are deleted (the result totals stay).
    pub snapshot_retention_days: i64,
    pub veteran: Option<VeteranConfig>,
    pub overall_active: Option<ActivityConfig>,
    pub active: Option<ActivityConfig>,
    pub very_active: Option<ActivityConfig>,
    pub crystal_pvper: Option<CrystalConfig>,
    pub builder: Option<BuilderConfig>,
    pub identity: IdentityConfig,
    pub texts: TextsConfig,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VeteranConfig {
    /// First join must be more than this many days before the poll started.
    pub days: i64,
}

/// At least `min_days` UTC days with at least `min_minutes` of play inside the
/// `window_days` whole days before the cut-off day.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityConfig {
    pub window_days: i64,
    pub min_days: i64,
    pub min_minutes: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CrystalConfig {
    pub window_days: i64,
    pub min_fights: i64,
    pub min_opponents: i64,
    /// Minimum total damage in a fight (both sides, tenths of HP) for it to count.
    pub min_damage: i64,
    /// Fights against the same opponent counted per UTC day.
    pub pair_day_cap: i64,
    /// The opponent's account must be at least this many days old.
    pub opponent_min_age_days: i64,
}

/// Builder, from the plugin's daily samples of the vanilla counters.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderConfig {
    pub window_days: i64,
    /// Days in the window with at least `min_day_placed` blocks placed.
    pub build_days: i64,
    pub min_day_placed: i64,
    /// Blocks placed in the whole window (obsidian is counted apart).
    pub placed: i64,
    /// Distinct block types placed at least `min_material_placed` times.
    pub materials: i64,
    pub min_material_placed: i64,
    /// Placed over mined, in tenths (15 = 1.5).
    pub placed_per_mined_tenths: i64,
    /// Obsidian over (placed + obsidian), in percent (50 = 0.5).
    pub max_obsidian_percent: i64,
}

/// One person, several accounts: IP-hash clustering.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Look at IP hashes seen in this many days before the cut-off.
    pub window_days: i64,
    /// An IP hash seen on more than this many accounts (VPN, cafe, carrier)
    /// links nobody.
    pub hub_limit: i64,
}

/// Plain-words noun phrases used in the poll text and in the private reason
/// for an ineligible click. They must never contain numbers.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextsConfig {
    pub crystal_pvper: String,
    pub veteran: String,
    pub builder: String,
    pub overall_active: String,
    pub active: String,
    pub very_active: String,
}

impl Default for TextsConfig {
    fn default() -> Self {
        Self {
            crystal_pvper: "recent crystal PvP on 6b6t".into(),
            veteran: "a long history on 6b6t".into(),
            builder: "recent building on 6b6t".into(),
            overall_active: "some recent play on 6b6t".into(),
            active: "regular recent play on 6b6t".into(),
            very_active: "very frequent recent play on 6b6t".into(),
        }
    }
}

/// True when `text` has a digit outside the brand name "6b6t".
pub fn contains_number(text: &str) -> bool {
    text.to_lowercase()
        .replace("6b6t", "")
        .chars()
        .any(|character| character.is_ascii_digit())
}

impl TextsConfig {
    pub fn phrase(&self, class: Class) -> &str {
        match class {
            Class::CrystalPvper => &self.crystal_pvper,
            Class::Veteran => &self.veteran,
            Class::Builder => &self.builder,
            Class::OverallActive => &self.overall_active,
            Class::Active => &self.active,
            Class::VeryActive => &self.very_active,
        }
    }
}

/// The JSON shape of `POLLS_CONFIG`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct RawPollConfig {
    #[serde(default)]
    show_turnout: bool,
    #[serde(default = "yes")]
    show_live_counts: bool,
    #[serde(default = "yes")]
    drop_departed_voters: bool,
    #[serde(default)]
    allow_partial_data: bool,
    #[serde(default = "default_min_duration_minutes")]
    min_duration_minutes: i64,
    #[serde(default = "default_max_duration_days")]
    max_duration_days: i64,
    #[serde(default = "default_retention_days")]
    snapshot_retention_days: i64,
    veteran: Option<VeteranConfig>,
    overall_active: Option<ActivityConfig>,
    active: Option<ActivityConfig>,
    very_active: Option<ActivityConfig>,
    crystal_pvper: Option<CrystalConfig>,
    builder: Option<BuilderConfig>,
    identity: IdentityConfig,
    #[serde(default)]
    texts: TextsConfig,
}

const fn yes() -> bool {
    true
}

const fn default_min_duration_minutes() -> i64 {
    10
}

const fn default_max_duration_days() -> i64 {
    30
}

const fn default_retention_days() -> i64 {
    365
}

impl PollConfig {
    /// Parses the JSON from `POLLS_CONFIG` or `POLLS_CONFIG_FILE`.
    ///
    /// `identity` is required. Each class is optional, but a class that is not
    /// listed cannot be used in a poll, and a listed class must give every
    /// value.
    pub fn from_json(json: &str) -> Result<Self> {
        let raw: RawPollConfig =
            serde_json::from_str(json).context("invalid polls configuration")?;
        let config = Self {
            show_turnout: raw.show_turnout,
            show_live_counts: raw.show_live_counts,
            drop_departed_voters: raw.drop_departed_voters,
            allow_partial_data: raw.allow_partial_data,
            min_duration_minutes: raw.min_duration_minutes,
            max_duration_days: raw.max_duration_days,
            snapshot_retention_days: raw.snapshot_retention_days,
            veteran: raw.veteran,
            overall_active: raw.overall_active,
            active: raw.active,
            very_active: raw.very_active,
            crystal_pvper: raw.crystal_pvper,
            builder: raw.builder,
            identity: raw.identity,
            texts: raw.texts,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let activity = [
            ("overall_active", &self.overall_active),
            ("active", &self.active),
            ("very_active", &self.very_active),
        ];
        for (name, tier) in activity {
            let Some(tier) = tier else {
                continue;
            };
            if tier.window_days < 1 || tier.min_days < 1 || tier.min_minutes < 1 {
                bail!("polls.{name}: window_days, min_days and min_minutes must be at least 1");
            }
            if tier.min_days > tier.window_days {
                bail!("polls.{name}: min_days cannot exceed window_days");
            }
        }
        if self.veteran.is_some_and(|veteran| veteran.days < 1) {
            bail!("polls.veteran.days must be at least 1");
        }
        if let Some(crystal) = &self.crystal_pvper
            && (crystal.window_days < 1
                || crystal.min_fights < 1
                || crystal.min_opponents < 1
                || crystal.min_damage < 0
                || crystal.pair_day_cap < 1
                || crystal.opponent_min_age_days < 0)
        {
            bail!("polls.crystal_pvper has an out-of-range value");
        }
        if let Some(builder) = &self.builder
            && (builder.window_days < 1
                || builder.build_days < 1
                || builder.build_days > builder.window_days
                || builder.min_day_placed < 0
                || builder.placed < 0
                || builder.materials < 0
                || builder.min_material_placed < 0
                || builder.placed_per_mined_tenths < 0
                || !(0..=100).contains(&builder.max_obsidian_percent))
        {
            bail!("polls.builder has an out-of-range value");
        }
        if self.identity.window_days < 1 || self.identity.hub_limit < 1 {
            bail!("polls.identity window_days and hub_limit must be at least 1");
        }
        if self.min_duration_minutes < 1 || self.max_duration_days < 1 {
            bail!("polls duration limits must be positive");
        }
        if self.snapshot_retention_days < 1 {
            bail!("polls.snapshot_retention_days must be at least 1");
        }
        for class in Class::ALL {
            let phrase = self.texts.phrase(class);
            if phrase.trim().is_empty() {
                bail!("polls.texts.{} must not be empty", class.name());
            }
            if contains_number(phrase) {
                bail!(
                    "polls.texts.{} must not contain numbers: players must not learn thresholds",
                    class.name()
                );
            }
        }
        Ok(())
    }

    /// Whether `class` has settings, i.e. can be used in a poll.
    #[cfg(test)]
    pub fn is_configured(&self, class: Class) -> bool {
        match class {
            Class::Veteran => self.veteran.is_some(),
            Class::OverallActive => self.overall_active.is_some(),
            Class::Active => self.active.is_some(),
            Class::VeryActive => self.very_active.is_some(),
            Class::CrystalPvper => self.crystal_pvper.is_some(),
            Class::Builder => self.builder.is_some(),
        }
    }
}

/// Arbitrary values for the tests. They are not the production values and say
/// nothing about them; production values live only in deployment config.
#[cfg(test)]
pub fn test_config() -> PollConfig {
    PollConfig {
        show_turnout: false,
        show_live_counts: true,
        drop_departed_voters: true,
        allow_partial_data: false,
        min_duration_minutes: 10,
        max_duration_days: 30,
        snapshot_retention_days: 365,
        veteran: Some(VeteranConfig { days: 100 }),
        overall_active: Some(ActivityConfig {
            window_days: 10,
            min_days: 2,
            min_minutes: 5,
        }),
        active: Some(ActivityConfig {
            window_days: 6,
            min_days: 3,
            min_minutes: 5,
        }),
        very_active: Some(ActivityConfig {
            window_days: 6,
            min_days: 4,
            min_minutes: 60,
        }),
        crystal_pvper: Some(CrystalConfig {
            window_days: 5,
            min_fights: 4,
            min_opponents: 2,
            min_damage: 30,
            pair_day_cap: 3,
            opponent_min_age_days: 2,
        }),
        builder: Some(BuilderConfig {
            window_days: 9,
            build_days: 3,
            min_day_placed: 200,
            placed: 3000,
            materials: 10,
            min_material_placed: 8,
            placed_per_mined_tenths: 12,
            max_obsidian_percent: 40,
        }),
        identity: IdentityConfig {
            window_days: 20,
            hub_limit: 3,
        },
        texts: TextsConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: &str = r#""identity": {"window_days": 20, "hub_limit": 3}"#;

    #[test]
    fn the_test_config_is_valid() {
        test_config().validate().unwrap();
    }

    #[test]
    fn no_threshold_is_built_into_the_code() {
        let config = PollConfig::from_json(&format!("{{{IDENTITY}}}")).unwrap();
        for class in Class::ALL {
            assert!(
                !config.is_configured(class),
                "{} must have no default",
                class.name()
            );
        }
        assert!(!config.show_turnout);
        assert!(config.show_live_counts);
    }

    #[test]
    fn identity_is_required() {
        assert!(PollConfig::from_json("{}").is_err());
    }

    #[test]
    fn configured_classes_must_be_complete() {
        let incomplete =
            format!(r#"{{{IDENTITY}, "active": {{"window_days": 6, "min_days": 3}}}}"#);
        let error = PollConfig::from_json(&incomplete).unwrap_err();
        assert!(format!("{error:#}").contains("min_minutes"), "{error:#}");

        let json = format!(
            r#"{{{IDENTITY}, "veteran": {{"days": 50}},
                "active": {{"window_days": 6, "min_days": 3, "min_minutes": 5}},
                "show_turnout": true}}"#
        );
        let config = PollConfig::from_json(&json).unwrap();
        assert_eq!(config.veteran, Some(VeteranConfig { days: 50 }));
        assert!(config.is_configured(Class::Active));
        assert!(!config.is_configured(Class::Builder));
        assert!(config.show_turnout);
    }

    #[test]
    fn rejects_typos_and_bad_values() {
        let bad = |extra: &str| PollConfig::from_json(&format!("{{{IDENTITY}, {extra}}}"));
        assert!(bad(r#""veterans": {}"#).is_err());
        assert!(bad(r#""veteran": {"day": 3}"#).is_err());
        assert!(bad(r#""veteran": {"days": 0}"#).is_err());
        assert!(bad(r#""active": {"window_days": 3, "min_days": 9, "min_minutes": 5}"#).is_err());
        assert!(
            PollConfig::from_json(r#"{"identity": {"window_days": 1, "hub_limit": 0}}"#).is_err()
        );
    }

    #[test]
    fn player_facing_texts_cannot_contain_numbers() {
        let with_text = |text: &str| {
            PollConfig::from_json(&format!(
                r#"{{{IDENTITY}, "texts": {{"veteran": "{text}"}}}}"#
            ))
        };
        assert!(with_text("180 days on 6b6t").is_err());
        assert!(with_text("").is_err());
        assert!(with_text("an old account on 6b6t").is_ok());
        assert!(contains_number("180 days on 6b6t"));
        assert!(!contains_number("a long history on 6b6t"));
    }
}
