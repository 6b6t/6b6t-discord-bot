//! Everything players read: the poll embed, the buttons and the private replies.
//!
//! None of it may contain a threshold or a number that hints at one. The
//! "who can vote" sentence is built from the plain-words phrases in
//! `PollConfig::texts`, never from the class parameters.

use std::{collections::HashSet, fmt::Write as _};

use poise::serenity_prelude as serenity;

use super::{
    config::TextsConfig,
    expr::{Expr, ParseError},
};

pub const MAX_OPTIONS: usize = 5;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_OPTION_CHARS: usize = 80;

const OPEN_COLOUR: u32 = 0x0058_65F2;
const CLOSED_COLOUR: u32 = 0x0099_AAB5;
const BAR_WIDTH: usize = 12;

/// Reason codes for `poll_denials` (counters only).
pub const DENIAL_NOT_ELIGIBLE: &str = "not_eligible";
pub const DENIAL_CLOSED: &str = "closed";

/// The plain-words description of who can vote, e.g.
/// `recent crystal PvP on 6b6t and a long history on 6b6t`.
pub fn who_phrase(expr: &Expr, texts: &TextsConfig) -> String {
    match expr {
        Expr::Class(call) => texts.phrase(call.class).to_owned(),
        Expr::Not(inner) => match inner.as_ref() {
            Expr::Class(call) => format!("no {}", texts.phrase(call.class)),
            other => format!("not ({})", who_phrase(other, texts)),
        },
        Expr::And(items) => join(items, " and ", texts),
        Expr::Or(items) => join(items, " or ", texts),
    }
}

fn join(items: &[Expr], separator: &str, texts: &TextsConfig) -> String {
    items
        .iter()
        .map(|item| match item {
            Expr::And(_) | Expr::Or(_) => format!("({})", who_phrase(item, texts)),
            _ => who_phrase(item, texts),
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// The private answer to an ineligible click. No numbers, no thresholds.
pub fn ineligible_reason(who: &str) -> String {
    format!(
        "You need a linked Minecraft account with {who} before this poll started. Who can vote was fixed when the poll started, so linking or playing now will not count."
    )
}

pub const CLOSED_REPLY: &str = "This poll is closed, so votes are not counted any more.";
pub const MISSING_REPLY: &str = "This poll could not be found.";
pub const STARTING_REPLY: &str = "Polls are still starting. Please try again shortly.";
pub const ERROR_REPLY: &str = "Something went wrong and your vote was not saved. Please try again.";

pub fn vote_reply(option: &str, first: bool, changed: bool) -> String {
    if first {
        format!("Your vote for **{option}** is counted. You can change it until the poll closes.")
    } else if changed {
        format!("Your vote is now **{option}**. You can change it until the poll closes.")
    } else {
        format!(
            "Your vote for **{option}** was already counted. You can change it until the poll closes."
        )
    }
}

/// Counts per option from the stored votes, ignoring `excluded` voters (people
/// who have left the server by the time the poll closes).
pub fn tally(option_count: usize, votes: &[(String, u8)], excluded: &HashSet<String>) -> Vec<u32> {
    let mut counts = vec![0; option_count];
    for (voter, option) in votes {
        if excluded.contains(voter) {
            continue;
        }
        if let Some(slot) = counts.get_mut(usize::from(*option)) {
            *slot += 1;
        }
    }
    counts
}

/// Whole percentages that add up to 100 (largest remainder), or all zeros for
/// no votes.
pub fn percentages(counts: &[u32]) -> Vec<u32> {
    let total: u32 = counts.iter().sum();
    if total == 0 {
        return vec![0; counts.len()];
    }
    let mut shares: Vec<(usize, u32, u32)> = counts
        .iter()
        .enumerate()
        .map(|(index, count)| (index, count * 100 / total, count * 100 % total))
        .collect();
    let assigned: u32 = shares.iter().map(|(_, whole, _)| whole).sum();
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by(|a, b| shares[*b].2.cmp(&shares[*a].2).then(a.cmp(b)));
    for index in order.into_iter().take((100 - assigned) as usize) {
        shares[index].1 += 1;
    }
    shares.sort_by_key(|(index, _, _)| *index);
    shares.into_iter().map(|(_, whole, _)| whole).collect()
}

fn bar(percent: u32) -> String {
    let filled = ((percent as usize * BAR_WIDTH) + 50) / 100;
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

/// What the embed needs to draw one poll.
pub struct PollView<'a> {
    pub poll_id: u64,
    pub title: &'a str,
    pub options: &'a [String],
    pub who: &'a str,
    /// Start of the activity window (epoch seconds) when a class has one.
    pub window_start: Option<i64>,
    pub cutoff_seconds: i64,
    pub ends_at: i64,
    pub closed: bool,
    /// Counts to draw; `None` hides them (live counts switched off).
    pub counts: Option<&'a [u32]>,
    /// Shown only when the turnout toggle is on.
    pub eligible: Option<i32>,
}

pub fn embed(view: &PollView<'_>) -> serenity::CreateEmbed {
    let mut description = format!(
        "Only players with a linked Minecraft account and {} can vote.\n",
        view.who
    );
    if let Some(window_start) = view.window_start {
        let _ = writeln!(
            description,
            "Activity counted: <t:{window_start}:D> to <t:{}:f>, before this poll started.",
            view.cutoff_seconds
        );
    } else {
        let _ = writeln!(
            description,
            "Activity counted: everything before <t:{}:f>, when this poll started.",
            view.cutoff_seconds
        );
    }
    description.push_str("Who can vote was fixed when the poll started.\n\n");
    if view.closed {
        description.push_str("**This poll is closed.** Final results:");
    } else {
        let _ = write!(
            description,
            "Press a button to vote. You can change your vote until it closes <t:{}:R>.",
            view.ends_at
        );
    }
    let mut embed = serenity::CreateEmbed::new()
        .title(view.title)
        .description(description)
        .colour(if view.closed {
            CLOSED_COLOUR
        } else {
            OPEN_COLOUR
        });
    let total = view.counts.map_or(0, |counts| counts.iter().sum::<u32>());
    if let Some(counts) = view.counts {
        let shares = percentages(counts);
        for (index, option) in view.options.iter().enumerate() {
            let count = counts.get(index).copied().unwrap_or(0);
            let share = shares.get(index).copied().unwrap_or(0);
            embed = embed.field(
                option,
                format!(
                    "`{}` {count} {} ({share}%)",
                    bar(share),
                    if count == 1 { "vote" } else { "votes" }
                ),
                false,
            );
        }
    } else if !view.closed {
        embed = embed.field("Results", "Shown when the poll closes.", false);
    }
    let mut footer = format!("Poll #{}", view.poll_id);
    if view.counts.is_some() {
        match view.eligible {
            Some(eligible) => {
                let _ = write!(footer, " · {total} of {eligible} eligible voted");
            }
            None => {
                let _ = write!(
                    footer,
                    " · {total} {} counted",
                    if total == 1 { "vote" } else { "votes" }
                );
            }
        }
    }
    embed.footer(serenity::CreateEmbedFooter::new(footer))
}

pub fn buttons(poll_id: u64, options: &[String]) -> Vec<serenity::CreateActionRow> {
    vec![serenity::CreateActionRow::Buttons(
        options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                serenity::CreateButton::new(format!("poll:v:{poll_id}:{index}"))
                    .label(option)
                    .style(serenity::ButtonStyle::Primary)
            })
            .collect(),
    )]
}

/// `poll:v:<poll_id>:<option>` to `(poll_id, option)`.
pub fn parse_button(custom_id: &str) -> Option<(u64, usize)> {
    let rest = custom_id.strip_prefix("poll:v:")?;
    let (poll_id, option) = rest.split_once(':')?;
    Some((poll_id.parse().ok()?, option.parse().ok()?))
}

/// The staff-facing results text: only counts of eligible votes, plus the
/// turnout and refused-click lines when the config toggle is on.
pub fn results_text(
    title: &str,
    options: &[String],
    counts: &[u32],
    eligible: Option<i32>,
    denials: &[(String, u32)],
    closed: bool,
) -> String {
    let shares = percentages(counts);
    let total: u32 = counts.iter().sum();
    let mut text = format!(
        "**{title}** ({})\n",
        if closed { "closed" } else { "still open" }
    );
    for (index, option) in options.iter().enumerate() {
        let _ = writeln!(
            text,
            "- {option}: {} ({}%)",
            counts.get(index).copied().unwrap_or(0),
            shares.get(index).copied().unwrap_or(0)
        );
    }
    let _ = write!(text, "Votes counted: {total}");
    if let Some(eligible) = eligible {
        let _ = write!(text, " of {eligible} eligible");
    }
    if !denials.is_empty() {
        let list: Vec<String> = denials
            .iter()
            .map(|(reason, count)| format!("{reason}: {count}"))
            .collect();
        let _ = write!(text, "\nRefused clicks ({}).", list.join(", "));
    }
    text
}

/// Validates the staff input shared by `/poll create` and its tests.
pub fn validate_poll_text(title: &str, options: &[String]) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "The question must be 1-{MAX_TITLE_CHARS} characters."
        ));
    }
    if options.len() < MIN_OPTIONS || options.len() > MAX_OPTIONS {
        return Err(format!("A poll needs {MIN_OPTIONS}-{MAX_OPTIONS} options."));
    }
    let mut seen = HashSet::new();
    for option in options {
        let trimmed = option.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_OPTION_CHARS {
            return Err(format!(
                "Each option must be 1-{MAX_OPTION_CHARS} characters."
            ));
        }
        if !seen.insert(trimmed.to_lowercase()) {
            return Err(format!("The option \"{trimmed}\" is listed twice."));
        }
    }
    Ok(())
}

pub fn requires_error(error: &ParseError) -> String {
    format!("The requires expression is not valid: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polls::{config::contains_number, expr::parse};

    fn texts() -> TextsConfig {
        TextsConfig::default()
    }

    #[test]
    fn who_phrase_follows_the_expression() {
        let single = parse("crystal_pvper").unwrap();
        assert_eq!(who_phrase(&single, &texts()), "recent crystal PvP on 6b6t");
        let both = parse("veteran AND active").unwrap();
        assert_eq!(
            who_phrase(&both, &texts()),
            "a long history on 6b6t and regular recent play on 6b6t"
        );
        let nested = parse("builder OR (veteran AND NOT active)").unwrap();
        assert_eq!(
            who_phrase(&nested, &texts()),
            "recent building on 6b6t or (a long history on 6b6t and no regular recent play on 6b6t)"
        );
    }

    #[test]
    fn parameters_never_reach_player_text() {
        let expr = parse("veteran(days=365) AND very_active(days=5, minutes=90)").unwrap();
        let phrase = who_phrase(&expr, &texts());
        assert!(!contains_number(&phrase));
        let reason = ineligible_reason(&phrase);
        assert!(!contains_number(&reason));
        for config_phrase in [
            texts().crystal_pvper,
            texts().veteran,
            texts().builder,
            texts().overall_active,
            texts().active,
            texts().very_active,
        ] {
            let reason = ineligible_reason(&config_phrase);
            assert!(!contains_number(&reason));
        }
    }

    #[test]
    fn the_ineligible_reason_matches_the_agreed_wording() {
        assert!(
            ineligible_reason("recent crystal PvP on 6b6t").starts_with(
                "You need a linked Minecraft account with recent crystal PvP on 6b6t before this poll started."
            )
        );
    }

    #[test]
    fn tally_ignores_departed_voters_and_bad_options() {
        let votes = vec![
            ("1".to_owned(), 0),
            ("2".to_owned(), 1),
            ("3".to_owned(), 1),
            ("4".to_owned(), 9),
        ];
        assert_eq!(tally(2, &votes, &HashSet::new()), vec![1, 2]);
        let gone: HashSet<String> = ["3".to_owned()].into();
        assert_eq!(tally(2, &votes, &gone), vec![1, 1]);
    }

    #[test]
    fn percentages_add_up_to_one_hundred() {
        assert_eq!(percentages(&[0, 0]), vec![0, 0]);
        assert_eq!(percentages(&[1, 1, 1]).iter().sum::<u32>(), 100);
        assert_eq!(percentages(&[3, 1]), vec![75, 25]);
        assert_eq!(percentages(&[21, 14, 2]).iter().sum::<u32>(), 100);
        assert_eq!(percentages(&[5]), vec![100]);
    }

    #[test]
    fn bars_scale_with_the_share() {
        assert_eq!(bar(0), "░".repeat(BAR_WIDTH));
        assert_eq!(bar(100), "█".repeat(BAR_WIDTH));
        assert_eq!(bar(50).chars().filter(|c| *c == '█').count(), BAR_WIDTH / 2);
    }

    #[test]
    fn button_ids_round_trip() {
        assert_eq!(parse_button("poll:v:12:3"), Some((12, 3)));
        assert_eq!(parse_button("poll:v:12"), None);
        assert_eq!(parse_button("poll:v:x:1"), None);
        assert_eq!(parse_button("events:apply"), None);
        let options: Vec<String> = ["a", "b"].iter().map(|s| (*s).to_owned()).collect();
        let rows = buttons(7, &options);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn staff_results_hide_turnout_unless_asked() {
        let options: Vec<String> = ["Faster", "Slower"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let hidden = results_text("Speed", &options, &[3, 1], None, &[], true);
        assert!(hidden.contains("- Faster: 3 (75%)"));
        assert!(hidden.contains("Votes counted: 4"));
        assert!(!hidden.contains("eligible"));
        let shown = results_text(
            "Speed",
            &options,
            &[3, 1],
            Some(40),
            &[("not_eligible".into(), 7)],
            false,
        );
        assert!(shown.contains("Votes counted: 4 of 40 eligible"));
        assert!(shown.contains("Refused clicks (not_eligible: 7)."));
    }

    #[test]
    fn poll_text_validation() {
        let two: Vec<String> = vec!["Yes".into(), "No".into()];
        assert!(validate_poll_text("Question?", &two).is_ok());
        assert!(validate_poll_text("", &two).is_err());
        assert!(validate_poll_text("Q", &two[..1]).is_err());
        let six: Vec<String> = (0..6).map(|i| format!("o{i}")).collect();
        assert!(validate_poll_text("Q", &six).is_err());
        let dup: Vec<String> = vec!["Yes".into(), "yes ".into()];
        assert!(validate_poll_text("Q", &dup).is_err());
        let long: Vec<String> = vec!["Yes".into(), "x".repeat(81)];
        assert!(validate_poll_text("Q", &long).is_err());
    }
}
