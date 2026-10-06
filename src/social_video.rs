use std::{collections::HashSet, time::Duration};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use poise::serenity_prelude as serenity;
use serde_json::{Value, json};

use crate::{config, youtube};

pub const DISCOVERY_INTERVAL: Duration = Duration::from_hours(6);
pub const PUBLICATION_INTERVAL: Duration = Duration::from_mins(20);

#[derive(Clone, Copy, Debug)]
pub enum Platform {
    Instagram,
    Tiktok,
}

impl Platform {
    fn actor(self) -> &'static str {
        match self {
            Self::Instagram => "apify~instagram-hashtag-scraper",
            Self::Tiktok => "clockworks~tiktok-hashtag-scraper",
        }
    }

    fn channel(self) -> serenity::ChannelId {
        match self {
            Self::Instagram => config::INSTAGRAM_ID,
            Self::Tiktok => config::TIKTOK_ID,
        }
    }

    fn input(self) -> Value {
        match self {
            Self::Instagram => json!({
                "hashtags": ["6b6t"], "resultsType": "reels",
                "resultsLimit": 5, "keywordSearch": false,
            }),
            Self::Tiktok => json!({
                "hashtags": ["6b6t"], "resultsPerPage": 5,
                "shouldDownloadVideos": false, "shouldDownloadCovers": false,
                "shouldDownloadSlideshowImages": false,
                "downloadSubtitlesOptions": "NEVER_DOWNLOAD_SUBTITLES",
            }),
        }
    }

    fn content_id(self) -> fn(&str) -> Option<&str> {
        match self {
            Self::Instagram => instagram_id,
            Self::Tiktok => tiktok_id,
        }
    }
}

#[derive(Clone)]
pub struct SocialVideoService {
    http: reqwest::Client,
    token: Option<String>,
}

impl SocialVideoService {
    pub fn new(http: reqwest::Client, token: Option<String>) -> Self {
        Self { http, token }
    }

    pub async fn notify(&self, ctx: &serenity::Context, platform: Platform) -> Result<()> {
        // No credentials means no paid requests or Discord mutations.
        let Some(token) = &self.token else {
            return Ok(());
        };
        // Publish pending announcements even if discovery subsequently fails.
        let posted = self.publish_pending(ctx, platform).await?;
        let items = self.fetch(platform, token).await?;
        let Some(video) = select_video(platform, &items, &posted, Utc::now()) else {
            return Ok(());
        };
        platform
            .channel()
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .content(video.message())
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send social video announcement")?;
        Ok(())
    }

    pub async fn publish_pending(
        &self,
        ctx: &serenity::Context,
        platform: Platform,
    ) -> Result<HashSet<String>> {
        if self.token.is_none() {
            return Ok(HashSet::new());
        }
        let messages = platform
            .channel()
            .messages(ctx, serenity::GetMessages::new().limit(100))
            .await
            .context("failed to load recent social video announcements")?;
        Ok(youtube::publish_due(ctx, &messages, platform.content_id()).await)
    }

    async fn fetch(&self, platform: Platform, token: &str) -> Result<Vec<Value>> {
        // Do not retry a POST: an ambiguous timeout may still have started a paid run.
        self.request(platform, token)
            .send()
            .await
            .context("Apify discovery request failed")?
            .error_for_status()
            .context("Apify discovery returned an error")?
            .json()
            .await
            .context("Apify discovery returned invalid JSON")
    }

    fn request(&self, platform: Platform, token: &str) -> reqwest::RequestBuilder {
        self.http
            .post(format!(
                "https://api.apify.com/v2/acts/{}/run-sync-get-dataset-items",
                platform.actor()
            ))
            .bearer_auth(token)
            .query(&[
                ("timeout", "180"),
                ("maxItems", "5"),
                ("maxTotalChargeUsd", "0.05"),
                ("restartOnError", "false"),
                ("format", "json"),
                ("clean", "true"),
                ("limit", "5"),
            ])
            .json(&platform.input())
            .timeout(Duration::from_secs(190))
    }
}

struct Video {
    id: String,
    caption: String,
    author: String,
    url: String,
    timestamp: DateTime<Utc>,
}

impl Video {
    fn message(&self) -> String {
        // Flatten untrusted captions so only our canonical video URL is on its own line.
        let caption: String = self
            .caption
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(400)
            .collect();
        let caption = caption.replace(['*', '_', '`', '~', '|', '[', ']', '<', '>'], "");
        format!("**{caption}** - {}\n{}", self.author, self.url)
    }
}

fn select_video(
    platform: Platform,
    items: &[Value],
    posted: &HashSet<String>,
    now: DateTime<Utc>,
) -> Option<Video> {
    items
        .iter()
        .filter_map(|item| parse_video(platform, item))
        .filter(|video| {
            let text = format!("{} {}", video.caption, video.author).to_ascii_lowercase();
            !posted.contains(&video.id)
                && video.caption.to_ascii_lowercase().contains("6b6t")
                && !youtube::IGNORE_WORDS.iter().any(|word| text.contains(word))
                // Hashtag feeds can return old/popular clips, unlike YouTube's date search.
                && video.timestamp <= now
                && video.timestamp >= now - chrono::Duration::days(30)
        })
        .max_by_key(|video| video.timestamp)
}

fn parse_video(platform: Platform, item: &Value) -> Option<Video> {
    if item.get("error").is_some() || item.get("errorCode").is_some() {
        return None;
    }
    let (caption, author, url, timestamp) = match platform {
        Platform::Instagram => {
            if item.get("type")?.as_str()? != "Video" {
                return None;
            }
            (
                item.get("caption")?,
                item.get("ownerUsername")?,
                item.get("url")?,
                item.get("timestamp")?,
            )
        }
        Platform::Tiktok => {
            if item
                .get("isSlideshow")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            (
                item.get("text")?,
                item.get("authorMeta")?.get("name")?,
                item.get("webVideoUrl")?,
                item.get("createTimeISO")?,
            )
        }
    };
    let author = author.as_str()?;
    if author.is_empty()
        || !author
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'))
    {
        return None;
    }
    let url = url.as_str()?;
    let id = platform.content_id()(url)?;
    let canonical_url = match platform {
        Platform::Instagram => format!("https://www.instagram.com/reel/{id}/"),
        Platform::Tiktok => {
            if !url.starts_with(&format!("https://www.tiktok.com/@{author}/video/")) {
                return None;
            }
            format!("https://www.tiktok.com/@{author}/video/{id}")
        }
    };
    Some(Video {
        id: id.to_owned(),
        caption: caption.as_str()?.to_owned(),
        author: author.to_owned(),
        url: canonical_url,
        timestamp: DateTime::parse_from_rfc3339(timestamp.as_str()?)
            .ok()?
            .with_timezone(&Utc),
    })
}

fn instagram_id(content: &str) -> Option<&str> {
    content.lines().find_map(|line| {
        let path = line.strip_prefix("https://www.instagram.com/")?;
        let path = path
            .strip_prefix("reel/")
            .or_else(|| path.strip_prefix("p/"))?;
        let id = path.trim_end_matches('/');
        (!id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
        .then_some(id)
    })
}

fn tiktok_id(content: &str) -> Option<&str> {
    content.lines().find_map(|line| {
        let path = line.strip_prefix("https://www.tiktok.com/@")?;
        let (author, id) = path.split_once("/video/")?;
        (!author.is_empty()
            && author
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'))
            && !id.is_empty()
            && id.chars().all(|c| c.is_ascii_digit()))
        .then_some(id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_and_publication_have_separate_intervals() {
        assert_eq!(DISCOVERY_INTERVAL.as_secs(), 6 * 60 * 60);
        assert_eq!(PUBLICATION_INTERVAL.as_secs(), 20 * 60);
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-06T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn requests_have_bearer_auth_and_enforced_run_limits() {
        crate::install_crypto_provider().unwrap();
        let service = SocialVideoService::new(reqwest::Client::new(), None);
        for platform in [Platform::Instagram, Platform::Tiktok] {
            let request = service.request(platform, "test-token").build().unwrap();
            assert_eq!(request.method(), reqwest::Method::POST);
            assert_eq!(request.headers()["authorization"], "Bearer test-token");
            assert!(!request.url().as_str().contains("test-token"));
            let query: std::collections::HashMap<_, _> = request.url().query_pairs().collect();
            assert_eq!(query["timeout"], "180");
            assert_eq!(query["maxItems"], "5");
            assert_eq!(query["maxTotalChargeUsd"], "0.05");
            assert_eq!(query["restartOnError"], "false");
            let body: Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            assert_eq!(body, platform.input());
        }
    }

    #[test]
    fn platform_channels_match_requested_announcement_channels() {
        assert_eq!(
            Platform::Instagram.channel().get(),
            1_557_078_627_152_035_911
        );
        assert_eq!(Platform::Tiktok.channel().get(), 1_557_078_855_288_881_272);
    }

    fn instagram(id: &str, caption: &str, date: &str) -> Value {
        json!({"type":"Video", "caption":caption, "ownerUsername":"creator",
            "url":format!("https://www.instagram.com/p/{id}/"), "timestamp":date})
    }

    #[test]
    fn selects_latest_eligible_video_and_skips_duplicates() {
        let items = vec![
            instagram("posted", "6b6t", "2026-10-06T11:00:00Z"),
            instagram("older", "6b6t", "2026-10-01T11:00:00Z"),
            instagram("newest", "6b6t base tour", "2026-10-05T11:00:00Z"),
            instagram("ignored", "6b6t versus 2b2t", "2026-10-06T11:00:00Z"),
        ];
        let posted = HashSet::from(["posted".to_owned()]);
        assert_eq!(
            select_video(Platform::Instagram, &items, &posted, now())
                .unwrap()
                .id,
            "newest"
        );
    }

    #[test]
    fn rejects_stale_future_unrelated_and_malformed_items() {
        let mut image = instagram("image", "6b6t", "2026-10-05T11:00:00Z");
        image["type"] = json!("Image");
        let items = vec![
            image,
            json!({"error":"no results", "errorCode":"NOT_FOUND"}),
            instagram("old", "6b6t", "2025-01-10T16:39:10Z"),
            instagram("future", "6b6t", "2026-10-07T11:00:00Z"),
            instagram("unrelated", "some other topic", "2026-10-05T11:00:00Z"),
            instagram("badtime", "6b6t", "invalid"),
        ];
        assert!(select_video(Platform::Instagram, &items, &HashSet::new(), now()).is_none());
    }

    #[test]
    fn parses_tiktok_output_and_disables_paid_downloads() {
        let item = json!({"text":"6b6t base tour", "authorMeta":{"name":"creator"},
            "webVideoUrl":"https://www.tiktok.com/@creator/video/123456",
            "createTimeISO":"2026-10-05T11:00:00Z"});
        let video = select_video(Platform::Tiktok, &[item], &HashSet::new(), now()).unwrap();
        assert_eq!(video.id, "123456");
        assert_eq!(Platform::Tiktok.input()["shouldDownloadVideos"], false);
        assert_eq!(
            Platform::Tiktok.input()["downloadSubtitlesOptions"],
            "NEVER_DOWNLOAD_SUBTITLES"
        );
        assert_eq!(Platform::Instagram.input()["resultsType"], "reels");
    }

    #[test]
    fn skips_tiktok_slideshows() {
        let item = json!({"text":"6b6t", "authorMeta":{"name":"creator"},
            "webVideoUrl":"https://www.tiktok.com/@creator/video/123456",
            "createTimeISO":"2026-10-05T11:00:00Z", "isSlideshow":true});
        assert!(parse_video(Platform::Tiktok, &item).is_none());
    }

    #[test]
    fn recovers_ids_only_from_valid_platform_urls() {
        assert_eq!(
            instagram_id("**6b6t** - creator\nhttps://www.instagram.com/reel/abc_123/"),
            Some("abc_123")
        );
        assert_eq!(
            instagram_id("https://www.instagram.com/p/abc_123/"),
            Some("abc_123")
        );
        assert_eq!(
            tiktok_id("**6b6t** - creator\nhttps://www.tiktok.com/@creator/video/123456"),
            Some("123456")
        );
        for url in [
            "https://evil.test/p/abc/",
            "https://www.instagram.com/p/abc/extra",
            "https://www.instagram.com/p/abc/?x=y",
        ] {
            assert_eq!(instagram_id(url), None);
        }
        assert_eq!(
            tiktok_id("https://www.tiktok.com/@creator/video/123?x=y"),
            None
        );
    }

    #[test]
    fn rejects_untrusted_urls_and_creator_mismatches() {
        let mut item = instagram("safe", "6b6t", "2026-10-05T11:00:00Z");
        item["url"] = json!("https://attacker.test/p/safe/");
        assert!(parse_video(Platform::Instagram, &item).is_none());
        let item = json!({"text":"6b6t", "authorMeta":{"name":"creator"},
            "webVideoUrl":"https://www.tiktok.com/@other/video/123456",
            "createTimeISO":"2026-10-05T11:00:00Z"});
        assert!(parse_video(Platform::Tiktok, &item).is_none());
    }

    #[test]
    fn captions_cannot_add_lines_or_exceed_discord_limit() {
        let mut item = instagram("safe", "6b6t\n**title**\n@everyone", "2026-10-05T11:00:00Z");
        let video = parse_video(Platform::Instagram, &item).unwrap();
        assert_eq!(video.message().lines().count(), 2);
        item["caption"] = json!("🦀".repeat(3000));
        assert!(
            parse_video(Platform::Instagram, &item)
                .unwrap()
                .message()
                .chars()
                .count()
                < 2000
        );
    }
}
