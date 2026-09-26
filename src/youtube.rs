use std::collections::HashSet;

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use serde::Deserialize;

const QUERIES: &[&str] = &["6b6t.org", "6b6t"];
const WHITELISTED_CHANNELS: &[&str] = &[
    "UCMLjKYJwRo7Z9-SkJUMw4rg",
    "UCBrOlHTLhY0dnmqBWqbD3IQ",
    "UCoXqVBjCPgKkoI_KZA3tutw",
];
const BLOCKED_CHANNELS: &[&str] = &[
    "UClo41vgAsX7YkhpxMW42WvA",
    "UCgs1Uk7zf_NQ4lEzSWwZGBQ", // @ak_mini_vlog-q2b
];
const IGNORE_WORDS: &[&str] = &[
    "2b2t",
    "5b5t",
    "7b7t",
    "9b9t",
    "constantiam",
    "8b8t",
    "leee",
    "jonarchy",
    "oldfag",
    "phoenixanarchy",
    "d2s9",
    "icecanarchy",
    "l2x9",
    "4b4t",
    "xbxt",
    "quiltanarchy",
    "0b0t",
    "cobblestone.com",
    "crashing",
    "botting",
    "bleepo",
];

#[derive(Clone)]
pub struct YoutubeService {
    http: reqwest::Client,
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    items: Vec<SearchResult>,
}

#[derive(Deserialize)]
struct SearchResult {
    id: Option<ResourceId>,
    snippet: Option<SearchResultSnippet>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourceId {
    video_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResultSnippet {
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    channel_title: String,
    #[serde(default)]
    channel_id: String,
}

struct YoutubeVideo {
    id: String,
    title: String,
    description: String,
    channel_title: String,
    channel_id: String,
}

impl YoutubeService {
    pub fn new(http: reqwest::Client, api_key: Option<String>) -> Self {
        Self { http, api_key }
    }

    pub async fn notify(
        &self,
        ctx: &serenity::Context,
        channel_id: serenity::ChannelId,
    ) -> Result<()> {
        let Some(api_key) = &self.api_key else {
            return Ok(());
        };
        let messages = channel_id
            .messages(ctx, serenity::GetMessages::new().limit(100))
            .await
            .context("failed to load recent YouTube announcements")?;
        let posted = publish_due(ctx, &messages).await;
        let response = self
            .http
            .get("https://www.googleapis.com/youtube/v3/search")
            .query(&[
                ("part", "snippet"),
                ("q", "6b6t.org OR 6b6t"),
                ("order", "date"),
                ("maxResults", "5"),
                ("type", "video"),
                ("key", api_key),
            ])
            .send()
            .await
            .context("YouTube search request failed")?
            .error_for_status()
            .context("YouTube search returned an error")?
            .json::<SearchResponse>()
            .await
            .context("YouTube search returned invalid JSON")?;
        let video = find_video(response.items, &posted);
        let Some(video) = video else { return Ok(()) };
        let title = html_escape::decode_html_entities(&video.title);
        let url = format!("https://www.youtube.com/watch?v={}", video.id);
        channel_id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(format!("**{title}** - {}\n{url}", video.channel_title))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await?;
        Ok(())
    }
}

async fn publish_due(ctx: &serenity::Context, messages: &[serenity::Message]) -> HashSet<String> {
    let cutoff = chrono::Utc::now().timestamp() - 12 * 60 * 60;
    // ponytail: 100 messages is one Discord request; persist IDs if this channel ever exceeds it between polls.
    let mut posted = HashSet::new();
    for message in messages
        .iter()
        .filter(|message| message.author.id == ctx.cache.current_user().id)
    {
        let Some(id) = youtube_video_id(&message.content) else {
            continue;
        };
        posted.insert(id.to_owned());
        if message.timestamp.unix_timestamp() <= cutoff
            && !message
                .flags
                .is_some_and(|flags| flags.contains(serenity::MessageFlags::CROSSPOSTED))
            && let Err(error) = message.crosspost(ctx).await
        {
            tracing::error!(%error, message_id = %message.id, "failed to publish YouTube announcement");
        }
    }
    posted
}

fn youtube_video_id(content: &str) -> Option<&str> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("https://www.youtube.com/watch?v="))
        .filter(|id| !id.is_empty())
}

fn find_video(items: Vec<SearchResult>, posted: &HashSet<String>) -> Option<YoutubeVideo> {
    items
        .into_iter()
        .filter_map(|item| {
            let snippet = item.snippet?;
            Some(YoutubeVideo {
                id: item.id?.video_id?,
                title: snippet.title,
                description: snippet.description,
                channel_title: snippet.channel_title,
                channel_id: snippet.channel_id,
            })
        })
        .find(|video| {
            if posted.contains(&video.id) || BLOCKED_CHANNELS.contains(&video.channel_id.as_str()) {
                return false;
            }
            let query_haystack =
                format!("{} {}", video.title, video.description).to_ascii_lowercase();
            let has_query = QUERIES
                .iter()
                .any(|query| query_haystack.contains(&query.to_ascii_lowercase()));
            let moderation_haystack =
                format!("{query_haystack} {}", video.channel_title).to_ascii_lowercase();
            let ignored = !WHITELISTED_CHANNELS.contains(&video.channel_id.as_str())
                && IGNORE_WORDS
                    .iter()
                    .any(|word| moderation_haystack.contains(word));
            has_query && !ignored
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_skips_posted_and_ignored_results() {
        let posted = HashSet::from(["posted".to_owned()]);
        let selected = find_video(
            vec![
                result("posted", "6b6t update", "channel"),
                result("ignored", "6b6t versus 2b2t", "channel"),
                result("selected", "6b6t base tour", "channel"),
            ],
            &posted,
        )
        .expect("an eligible result should be selected");

        assert_eq!(selected.id, "selected");
    }

    #[test]
    fn whitelisted_channels_can_use_other_server_names() {
        let selected = find_video(
            vec![result(
                "whitelisted",
                "6b6t and 2b2t comparison",
                WHITELISTED_CHANNELS[0],
            )],
            &HashSet::new(),
        )
        .expect("a whitelisted result should be selected");

        assert_eq!(selected.id, "whitelisted");
    }

    #[test]
    fn blocked_channels_are_never_selected() {
        for channel_id in BLOCKED_CHANNELS {
            let selected = find_video(
                vec![
                    result("blocked", "6b6t base tour", channel_id),
                    result("allowed", "6b6t base tour", "allowed-channel"),
                ],
                &HashSet::new(),
            )
            .expect("the allowed result should be selected");

            assert_eq!(selected.id, "allowed");
        }
    }

    #[test]
    fn channel_name_alone_does_not_match_a_query() {
        let mut unrelated = result("unrelated", "A completely unrelated video", "channel");
        unrelated.snippet.as_mut().unwrap().channel_title = "6b6t creator".to_owned();

        assert!(find_video(vec![unrelated], &HashSet::new()).is_none());
    }

    #[test]
    fn announcement_video_ids_are_recovered_from_discord_content() {
        assert_eq!(
            youtube_video_id("**Title** - Creator\nhttps://www.youtube.com/watch?v=abc123"),
            Some("abc123")
        );
        assert_eq!(youtube_video_id("unrelated"), None);
    }

    fn result(id: &str, title: &str, channel_id: &str) -> SearchResult {
        SearchResult {
            id: Some(ResourceId {
                video_id: Some(id.to_owned()),
            }),
            snippet: Some(SearchResultSnippet {
                title: title.to_owned(),
                description: String::new(),
                channel_id: channel_id.to_owned(),
                channel_title: "Creator".to_owned(),
            }),
        }
    }
}
