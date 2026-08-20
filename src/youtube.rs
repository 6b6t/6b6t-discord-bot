use std::{collections::HashSet, path::PathBuf, sync::Arc};

use anyhow::{Context as _, Result};
use google_youtube3::{
    YouTube,
    api::SearchResult,
    common::NoToken,
    hyper_rustls::{HttpsConnector, HttpsConnectorBuilder},
    hyper_util::{
        client::legacy::{Client, connect::HttpConnector},
        rt::TokioExecutor,
    },
};
use poise::serenity_prelude as serenity;
use tokio::sync::Mutex;

type YoutubeHub = YouTube<HttpsConnector<HttpConnector>>;

const QUERIES: &[&str] = &["6b6t.org", "6b6t"];
const WHITELISTED_CHANNELS: &[&str] = &[
    "UCMLjKYJwRo7Z9-SkJUMw4rg",
    "UCBrOlHTLhY0dnmqBWqbD3IQ",
    "UCoXqVBjCPgKkoI_KZA3tutw",
];
const BLOCKED_CHANNELS: &[&str] = &["UClo41vgAsX7YkhpxMW42WvA"];
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
    hub: Arc<YoutubeHub>,
    api_key: Option<String>,
    posted: Arc<Mutex<Option<HashSet<String>>>>,
    path: PathBuf,
}

struct YoutubeVideo {
    id: String,
    title: String,
    description: String,
    channel_title: String,
    channel_id: String,
}

impl YoutubeService {
    pub fn new(api_key: Option<String>) -> Result<Self> {
        let connector = HttpsConnectorBuilder::new()
            .with_native_roots()
            .context("failed to load native root certificates for YouTube")?
            .https_only()
            .enable_http2()
            .build();
        let client = Client::builder(TokioExecutor::new()).build(connector);
        Ok(Self {
            hub: Arc::new(YouTube::new(client, NoToken)),
            api_key,
            posted: Arc::new(Mutex::new(None)),
            path: PathBuf::from("data/youtube-posted.json"),
        })
    }

    pub async fn notify(
        &self,
        ctx: &serenity::Context,
        channel_id: serenity::ChannelId,
    ) -> Result<()> {
        let Some(api_key) = &self.api_key else {
            return Ok(());
        };
        let parts = vec!["snippet".to_owned()];
        let (_, response) = self
            .hub
            .search()
            .list(&parts)
            .q("6b6t.org OR 6b6t")
            .order("date")
            .max_results(5)
            .add_type("video")
            .param("key", api_key)
            .clear_scopes()
            .doit()
            .await
            .context("YouTube search request failed")?;
        let mut posted_guard = self.posted.lock().await;
        if posted_guard.is_none() {
            *posted_guard = Some(load_posted(&self.path).await?);
        }
        let posted = posted_guard.as_mut().expect("posted set was initialized");
        let video = find_video(response.items.unwrap_or_default(), posted);
        let Some(video) = video else { return Ok(()) };
        let title = html_escape::decode_html_entities(&video.title);
        let url = format!("https://www.youtube.com/watch?v={}", video.id);
        let message = channel_id
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(format!("**{title}** - {}\n{url}", video.channel_title))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await?;
        // Only suppress the video after Discord has accepted the message. A
        // transient send failure must remain eligible for the next poll.
        posted.insert(video.id.clone());
        save_posted(&self.path, posted).await?;
        drop(posted_guard);
        let http = ctx.http.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_hours(12)).await;
            if let Err(error) = message.crosspost(&http).await {
                tracing::error!(%error, "failed to publish YouTube announcement");
            }
        });
        Ok(())
    }
}

fn find_video(items: Vec<SearchResult>, posted: &HashSet<String>) -> Option<YoutubeVideo> {
    items
        .into_iter()
        .filter_map(|item| {
            let id = item.id?.video_id?;
            let snippet = item.snippet?;
            Some(YoutubeVideo {
                id,
                title: snippet.title.unwrap_or_default(),
                description: snippet.description.unwrap_or_default(),
                channel_title: snippet.channel_title.unwrap_or_default(),
                channel_id: snippet.channel_id.unwrap_or_default(),
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

async fn load_posted(path: &PathBuf) -> Result<HashSet<String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => serde_json::from_str::<Vec<String>>(&content)
            .map(|ids| ids.into_iter().collect())
            .context("invalid YouTube storage file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(error) => Err(error).context("failed to read YouTube storage file"),
    }
}
async fn save_posted(path: &PathBuf, posted: &HashSet<String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut values = posted.iter().cloned().collect::<Vec<_>>();
    values.sort();
    tokio::fs::write(path, serde_json::to_string_pretty(&values)?)
        .await
        .context("failed to save YouTube storage")
}

#[cfg(test)]
mod tests {
    use google_youtube3::api::{ResourceId, SearchResultSnippet};

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
        let selected = find_video(
            vec![
                result("blocked", "6b6t base tour", BLOCKED_CHANNELS[0]),
                result("allowed", "6b6t base tour", "allowed-channel"),
            ],
            &HashSet::new(),
        )
        .expect("the allowed result should be selected");

        assert_eq!(selected.id, "allowed");
    }

    #[test]
    fn channel_name_alone_does_not_match_a_query() {
        let mut unrelated = result("unrelated", "A completely unrelated video", "channel");
        unrelated.snippet.as_mut().expect("snippet").channel_title =
            Some("6b6t creator".to_owned());

        assert!(find_video(vec![unrelated], &HashSet::new()).is_none());
    }

    fn result(id: &str, title: &str, channel_id: &str) -> SearchResult {
        SearchResult {
            id: Some(ResourceId {
                video_id: Some(id.to_owned()),
                ..Default::default()
            }),
            snippet: Some(SearchResultSnippet {
                title: Some(title.to_owned()),
                channel_id: Some(channel_id.to_owned()),
                channel_title: Some("Creator".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}
