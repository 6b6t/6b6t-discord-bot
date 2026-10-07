use super::{ANNOUNCEMENTS, EVENT_ROLE, REVIEWS};
use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Days, NaiveDate, TimeZone as _, Utc};
use chrono_tz::Europe::Warsaw;
use serde_json::Value;

pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS banner_gateway_identifies (id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY, attempted_at BIGINT NOT NULL, INDEX banner_identify_time(attempted_at)) ENGINE=InnoDB",
    "CREATE TABLE IF NOT EXISTS banner_themes (year INT NOT NULL, month INT UNSIGNED NOT NULL, theme VARCHAR(200) NOT NULL, PRIMARY KEY(year,month)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "CREATE TABLE IF NOT EXISTS banner_contests (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, contest_key VARCHAR(64) NOT NULL UNIQUE, year INT NOT NULL, month INT UNSIGNED NOT NULL, state VARCHAR(24) NOT NULL DEFAULT 'scheduled', call_at BIGINT NOT NULL, close_at BIGINT NOT NULL, voting_at BIGINT NOT NULL, end_at BIGINT NOT NULL, dry_run BOOLEAN NOT NULL DEFAULT FALSE, reminded BOOLEAN NOT NULL DEFAULT FALSE, theme VARCHAR(200) NULL, call_message_id VARCHAR(32) NULL, effects LONGTEXT NOT NULL DEFAULT '{}', INDEX banner_due(state,call_at)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "CREATE TABLE IF NOT EXISTS banner_submissions (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, contest_id BIGINT UNSIGNED NOT NULL, discord_id VARCHAR(32) NOT NULL, username VARCHAR(17) NOT NULL, uuid CHAR(36) NOT NULL, email VARCHAR(254) NOT NULL, prize VARCHAR(16) NOT NULL, image MEDIUMBLOB NOT NULL, status VARCHAR(16) NOT NULL DEFAULT 'pending', decider VARCHAR(32) NULL, reason VARCHAR(500) NULL, submitted_at BIGINT NOT NULL, shuffle_key CHAR(36) NOT NULL, review_message_id VARCHAR(32) NULL, vote_message_id VARCHAR(32) NULL, votes BIGINT UNSIGNED NOT NULL DEFAULT 0, revision INT UNSIGNED NOT NULL DEFAULT 0, review_revision INT UNSIGNED NOT NULL DEFAULT 0, review_closed BOOLEAN NOT NULL DEFAULT FALSE, UNIQUE KEY banner_account(contest_id,discord_id), UNIQUE KEY banner_username(contest_id,username), UNIQUE KEY banner_uuid(contest_id,uuid), FOREIGN KEY(contest_id) REFERENCES banner_contests(id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci",
    "CREATE TABLE IF NOT EXISTS banner_winners (contest_id BIGINT UNSIGNED NOT NULL PRIMARY KEY, submission_id BIGINT UNSIGNED NOT NULL, year INT NOT NULL, month INT UNSIGNED NOT NULL, username VARCHAR(17) NOT NULL, uuid CHAR(36) NOT NULL, discord_id VARCHAR(32) NOT NULL, email VARCHAR(254) NOT NULL, prize VARCHAR(16) NOT NULL, expires_at BIGINT NOT NULL, image MEDIUMBLOB NOT NULL, votes BIGINT UNSIGNED NOT NULL, dry_run BOOLEAN NOT NULL DEFAULT FALSE, FOREIGN KEY(contest_id) REFERENCES banner_contests(id), FOREIGN KEY(submission_id) REFERENCES banner_submissions(id)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
];

#[derive(Clone, sqlx::FromRow)]
pub struct Contest {
    pub id: u64,
    pub year: i32,
    pub month: u32,
    pub state: String,
    pub call_at: i64,
    pub close_at: i64,
    pub voting_at: i64,
    pub end_at: i64,
    pub dry_run: bool,
    pub reminded: bool,
    pub theme: Option<String>,
    pub call_message_id: Option<String>,
}
impl Contest {
    pub fn channel(&self) -> u64 {
        if self.dry_run { REVIEWS } else { ANNOUNCEMENTS }
    }
    pub fn month_name(&self) -> &'static str {
        MONTHS[usize::try_from(self.month.saturating_sub(1))
            .unwrap_or(0)
            .min(11)]
    }
    pub fn date(&self) -> String {
        DateTime::from_timestamp(self.end_at, 0).map_or_else(String::new, |d| {
            d.with_timezone(&Warsaw).format("%Y/%m/%d").to_string()
        })
    }
}
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

#[derive(Clone, sqlx::FromRow)]
pub struct Entry {
    pub id: u64,
    pub contest_id: u64,
    pub discord_id: String,
    pub username: String,
    pub uuid: String,
    pub email: String,
    pub status: String,
    pub decider: Option<String>,
    pub reason: Option<String>,
    pub submitted_at: i64,
    pub shuffle_key: String,
    pub review_message_id: Option<String>,
    pub vote_message_id: Option<String>,
    pub votes: u64,
    pub revision: u32,
    pub review_revision: u32,
    pub review_closed: bool,
}

#[derive(Clone, Copy)]
pub struct Schedule {
    pub call: i64,
    pub close: i64,
    pub voting: i64,
    pub end: i64,
}
impl Schedule {
    pub fn monthly(year: i32, month: u32) -> Result<Self> {
        Self::ending(NaiveDate::from_ymd_opt(year, month, 1).context("invalid year/month")?)
    }
    pub fn ending(end: NaiveDate) -> Result<Self> {
        fn at(date: NaiveDate, hour: u32) -> Result<i64> {
            Ok(Warsaw
                .from_local_datetime(&date.and_hms_opt(hour, 0, 0).context("invalid time")?)
                .single()
                .context("ambiguous Warsaw time")?
                .timestamp())
        }
        let before = |days| {
            end.checked_sub_days(Days::new(days))
                .context("invalid date offset")
        };
        Ok(Self {
            call: at(before(7)?, 22)?,
            close: at(before(4)?, 22)?,
            voting: at(before(3)?, 10)?,
            end: at(end, 10)?,
        })
    }
    #[cfg(test)]
    pub fn test(now: i64) -> Self {
        Self::test_with(now, 1)
    }
    /// Test schedule with `minutes` per phase (clamped to 1-30), so staff have time to fill in the form.
    pub fn test_with(now: i64, minutes: u32) -> Self {
        let step = i64::from(minutes.clamp(1, 30)) * 60;
        Self {
            call: now,
            close: now + step,
            voting: now + 2 * step,
            end: now + 3 * step,
        }
    }
}

pub fn theme_reminder_at(call_at: i64) -> Result<i64> {
    DateTime::from_timestamp(call_at, 0)
        .context("invalid call timestamp")?
        .with_timezone(&Warsaw)
        .checked_sub_days(Days::new(2))
        .map(|date| date.timestamp())
        .context("invalid theme reminder date")
}

pub fn month_expiry(now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    // LuckPerms DurationParser uses Java ChronoUnit.MONTHS.getDuration(),
    // an average Gregorian month (2,629,746 seconds), not calendar addition.
    now.checked_add_signed(chrono::Duration::seconds(2_629_746))
        .context("prize expiry out of range")
}

pub fn prize(ranks: &[String]) -> Option<&'static str> {
    let ranks: Vec<String> = ranks.iter().map(|r| r.to_ascii_lowercase()).collect();
    for group in [
        "legendultra",
        "legend",
        "apex",
        "eliteultra",
        "elite+",
        "elite",
        "primeultra",
        "prime+",
        "prime",
    ] {
        if ranks.iter().any(|r| r == group) {
            return match group {
                "apex" => Some("legend"),
                "elite+" | "elite" => Some("eliteultra"),
                "prime+" | "prime" => Some("primeultra"),
                _ => None,
            };
        }
    }
    None
}

pub fn rank_name(rank: &str) -> &'static str {
    match rank {
        "primeultra" => "Prime Ultra",
        "eliteultra" => "Elite Ultra",
        _ => "Legend",
    }
}
pub fn winner_order(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    b.votes
        .cmp(&a.votes)
        .then_with(|| a.submitted_at.cmp(&b.submitted_at))
        .then_with(|| a.id.cmp(&b.id))
}

pub struct Application {
    pub username: String,
    pub email: String,
    pub url: String,
    pub size: u64,
}
// Modal labels use singular 'component'; legacy action rows use plural 'components'.
pub fn field<'a>(components: &'a Value, id: &str) -> Option<&'a Value> {
    for component in components.as_array()? {
        if component["custom_id"] == id {
            return Some(component);
        }
        if let Some(child) = component.get("component")
            && child["custom_id"] == id
        {
            return Some(child);
        }
        if let Some(children) = component.get("components")
            && let Some(found) = field(children, id)
        {
            return Some(found);
        }
    }
    None
}
pub fn parse_application(data: &Value) -> Result<Application> {
    let components = &data["components"];
    let text = |id| {
        field(components, id)
            .and_then(|f| f["value"].as_str())
            .map(|s| s.trim().to_owned())
            .context("missing text input")
    };
    let upload = field(components, "image").context("missing file upload")?;
    let values = upload["values"]
        .as_array()
        .context("missing attachment IDs")?;
    if upload["type"] != 19 || values.len() != 1 {
        bail!("exactly one image required");
    }
    let id = values[0].as_str().context("invalid attachment ID")?;
    let attachment = &data["resolved"]["attachments"][id];
    let url = attachment["url"]
        .as_str()
        .context("missing resolved attachment")?
        .to_owned();
    // Only Discord's attachment CDN, without redirects, may be fetched.
    let parsed = reqwest::Url::parse(&url)?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("cdn.discordapp.com" | "media.discordapp.net")
        )
        || !(parsed.path().starts_with("/attachments/")
            || parsed.path().starts_with("/ephemeral-attachments/"))
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port_or_known_default() != Some(443)
    {
        bail!("invalid attachment URL");
    }
    Ok(Application {
        username: text("username")?,
        email: text("email")?,
        url,
        size: attachment["size"]
            .as_u64()
            .context("missing attachment size")?,
    })
}
pub fn valid_username(name: &str) -> bool {
    let name = name.strip_prefix('.').unwrap_or(name);
    (3..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
pub fn valid_email(email: &str) -> bool {
    if email.len() > 254 || !email.is_ascii() || email.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b))
        && domain.contains('.')
        && !domain.contains('@')
        && domain.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

pub fn call_text(c: &Contest) -> String {
    let theme = c.theme.as_ref().map_or_else(
        || "Any screenshot taken on 6b6t is allowed.".to_owned(),
        |t| format!("**Theme: {t}.** Only screenshots in this theme will be allowed."),
    );
    format!(
        "## 6b6t Discord Banner [{}]\nSend us your best screenshot for the 6b6t Discord banner of **{}**!\n{theme}\nRequirements:\n- high render distance\n- shaders\n- taken on 6b6t\nThe winner gets **Prime Ultra**, **Elite Ultra** or **Legend** for 1 month (Prime → Prime Ultra, Elite → Elite Ultra, Apex → Legend), and their screenshot becomes the banner of the 6b6t Discord.\nClick **Apply** to send your screenshot. Submissions close <t:{}:F>.\n-# <@&{EVENT_ROLE}>",
        c.date(),
        c.month_name(),
        c.close_at
    )
}
pub fn voting_text(c: &Contest) -> String {
    format!(
        "## 6b6t Discord Banner [{}]\nWhich screenshot do you like the most? React with 🔥 on your favourites. The winner becomes the banner of the 6b6t Discord for **{}**. Voting ends <t:{}:R>.\n-# <@&{EVENT_ROLE}>",
        c.date(),
        c.month_name(),
        c.end_at
    )
}
pub fn winner_text(c: &Contest, e: &Entry, rank: &str) -> String {
    format!(
        "## 6b6t Discord Banner [{}]\nThe winner of the {} banner is **{}** (<@{}>) with **{}** 🔥! They get **{}** for 1 month. Thanks to everyone who took part.\n-# <@&{EVENT_ROLE}>",
        c.date(),
        c.month_name(),
        e.username,
        e.discord_id,
        e.votes,
        rank_name(rank)
    )
}

pub fn user_id(interaction: &Value) -> Option<&str> {
    interaction["member"]["user"]["id"]
        .as_str()
        .or_else(|| interaction["user"]["id"].as_str())
}
pub fn reviewer(interaction: &Value) -> bool {
    interaction["member"]["roles"]
        .as_array()
        .is_some_and(|roles| {
            roles.iter().any(|r| {
                matches!(
                    r.as_str(),
                    Some(
                        "917520262939938915"
                            | "1324344058138726481"
                            | "1268946626387378189"
                            | "1533970494284365854"
                    )
                )
            })
        })
}
pub fn can_review(c: &Contest, now: i64) -> bool {
    matches!(c.state.as_str(), "open" | "review") && now < c.voting_at
}
pub fn apply_modal(id: u64) -> Value {
    serde_json::json!({"type":9,"data":{"custom_id":format!("banner:form:{id}"),"title":"Apply for the Discord banner","components":[
        {"type":18,"label":"Minecraft username","component":{"type":4,"custom_id":"username","style":1,"required":true,"min_length":3,"max_length":17}},
        {"type":18,"label":"Email","component":{"type":4,"custom_id":"email","style":1,"required":true,"max_length":254}},
        {"type":18,"label":"Screenshot","component":{"type":19,"custom_id":"image","file_types":["image"],"min_values":1,"max_values":1,"required":true}}
    ]}})
}
pub fn review_components(id: u64, disabled: bool) -> Value {
    serde_json::json!([
        {"type":1,"components":[{"type":2,"style":3,"label":"Approve","custom_id":format!("banner:approve:{id}"),"disabled":disabled}]},
        {"type":1,"components":[{"type":3,"custom_id":format!("banner:deny:{id}"),"placeholder":"Deny","disabled":disabled,"options":[
            {"label":"Low quality","value":"Low quality"},{"label":"Off-topic","value":"Off-topic"},{"label":"Inappropriate","value":"Inappropriate"},{"label":"Other","value":"Other"}
        ]}]}
    ])
}
