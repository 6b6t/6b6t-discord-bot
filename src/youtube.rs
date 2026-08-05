use std::{collections::HashSet, path::PathBuf, sync::Arc};

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use serde::Deserialize;
use tokio::sync::Mutex;

const QUERIES: &[&str] = &["6b6t.org", "6b6t"];
const WHITELISTED_CHANNELS: &[&str] = &[
    "UCMLjKYJwRo7Z9-SkJUMw4rg",
    "UCBrOlHTLhY0dnmqBWqbD3IQ",
    "UCoXqVBjCPgKkoI_KZA3tutw",
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
    posted: Arc<Mutex<Option<HashSet<String>>>>,
    path: PathBuf,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    items: Vec<SearchItem>,
}
#[derive(Deserialize)]
struct SearchItem {
    id: SearchId,
    snippet: SearchSnippet,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchId {
    video_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchSnippet {
    title: String,
    description: String,
    channel_title: String,
    channel_id: String,
}

impl YoutubeService {
    pub fn new(http: reqwest::Client, api_key: Option<String>) -> Self {
        Self {
            http,
            api_key,
            posted: Arc::new(Mutex::new(None)),
            path: PathBuf::from("data/youtube-posted.json"),
        }
    }

    pub async fn notify(
        &self,
        ctx: &serenity::Context,
        channel_id: serenity::ChannelId,
    ) -> Result<()> {
        let Some(api_key) = &self.api_key else {
            return Ok(());
        };
        let response = self
            .http
            .get("https://www.googleapis.com/youtube/v3/search")
            .query(&[
                ("key", api_key.as_str()),
                ("part", "snippet"),
                ("order", "date"),
                ("maxResults", "5"),
                ("type", "video"),
                ("q", "6b6t.org OR 6b6t"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<SearchResponse>()
            .await?;
        let mut posted_guard = self.posted.lock().await;
        if posted_guard.is_none() {
            *posted_guard = Some(load_posted(&self.path).await?);
        }
        let posted = posted_guard.as_mut().expect("posted set was initialized");
        let video = response.items.into_iter().find(|item| {
            let Some(id) = &item.id.video_id else {
                return false;
            };
            if posted.contains(id) {
                return false;
            }
            let haystack = format!(
                "{} {} {}",
                item.snippet.title, item.snippet.description, item.snippet.channel_title
            )
            .to_ascii_lowercase();
            let has_query = QUERIES
                .iter()
                .any(|query| haystack.contains(&query.to_ascii_lowercase()));
            let ignored = !WHITELISTED_CHANNELS.contains(&item.snippet.channel_id.as_str())
                && IGNORE_WORDS.iter().any(|word| haystack.contains(word));
            has_query && !ignored
        });
        let Some(video) = video else { return Ok(()) };
        let video_id = video
            .id
            .video_id
            .context("selected YouTube item had no video ID")?;
        posted.insert(video_id.clone());
        save_posted(&self.path, posted).await?;
        drop(posted_guard);
        let title = html_escape::decode_html_entities(&video.snippet.title);
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        let message = channel_id
            .say(
                ctx,
                format!("**{title}** - {}\n{url}", video.snippet.channel_title),
            )
            .await?;
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
