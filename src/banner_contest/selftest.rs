use super::*;
use chrono::Timelike as _;

pub(super) const TEST_PLAYER: &str = "BannerSelftest";

struct OriginalImage {
    field: &'static str,
    bytes: Vec<u8>,
    dimensions: (u32, u32),
}

fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    Ok(::image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()?)
}

fn original_uri(bytes: &[u8]) -> Result<String> {
    let mime = match ::image::guess_format(bytes)? {
        ::image::ImageFormat::Png => "image/png",
        ::image::ImageFormat::Jpeg => "image/jpeg",
        ::image::ImageFormat::WebP => "image/webp",
        _ => bail!("Unsupported original image format"),
    };
    if bytes.len() >= image::MAX_GUILD_IMAGE {
        bail!("Original too large to restore automatically");
    }
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

impl BannerService {
    async fn selftest_step(&self, id: u64, key: &str, text: &str) -> Result<()> {
        self.queue_report(id, key, &format!("Banner selftest: {text}"))
            .await?;
        self.flush_reports().await
    }

    pub(super) async fn selftest(
        &self,
        server: &ServerService,
        user: u64,
        username: &str,
        group: &str,
    ) -> Result<()> {
        let _guard = self.lock.lock().await;
        let connection = self
            .acquire()
            .await?
            .context("Contest worker busy; try again.")?;
        let result = async {
            let active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM banner_contests WHERE state IN ('open','review','voting')",
            )
            .fetch_one(&self.pool)
            .await?;
            if active > 0 {
                bail!("Selftest refused while a real contest is active");
            }
            let now = Utc::now().with_timezone(&chrono_tz::Europe::Warsaw);
            if now.day() == 1 && (9..=10).contains(&now.hour()) {
                bail!("Selftest refused near monthly contest generation");
            }
            let id = self
                .operational_journal(&format!("selftest-{}", uuid::Uuid::new_v4()))
                .await?;
            self.selftest_step(
                id,
                "start",
                "Started; all reports and notification fallback stay in banner-reviews.",
            )
            .await?;
            let result = self.selftest_inner(id, server, user, username, group).await;
            let finish = match &result {
                Ok(()) => "Completed.".to_owned(),
                Err(error) => {
                    // Service error chains may contain credentialed URLs; log only our own context.
                    let detail = if error.downcast_ref::<reqwest::Error>().is_some() {
                        "Service transport or HTTP failure".to_owned()
                    } else {
                        error.to_string()
                    };
                    format!(
                        "FAILED: {detail}. No automatic re-dispatch of guild or prize commands."
                    )
                }
            };
            self.selftest_step(id, "finish", &finish).await?;
            result
        }
        .await;
        Self::release(connection).await;
        result
    }

    async fn selftest_inner(
        &self,
        id: u64,
        server: &ServerService,
        user: u64,
        username: &str,
        group: &str,
    ) -> Result<()> {
        // Refuse an existing prize, including inherited groups: removal must never touch it.
        if !username.eq_ignore_ascii_case(TEST_PLAYER) {
            bail!("Selftest is restricted to BannerSelftest");
        }
        if !matches!(group, "primeultra" | "eliteultra") {
            bail!("invalid prize group");
        }
        let uuid = server
            .banner_uuid(username)
            .await?
            .context("Username must resolve uniquely")?;
        let ranks = server
            .ranks(username)
            .await?
            .context("Username not found")?;
        if ranks.iter().any(|r| r.eq_ignore_ascii_case(group)) {
            bail!("Requested prize already present; choose another group or account")
        }
        self.selftest_step(
            id,
            "identity",
            "UUID resolved through the winner path; requested prize absent.",
        )
        .await?;

        let images = self.selftest_guild_images(id).await;
        let images_report = self
            .selftest_step(
                id,
                "images_result",
                if images.is_ok() {
                    "Images completed or safely skipped."
                } else {
                    "Images FAILED; inspect image reports and backup. Rank and DM tests continue."
                },
            )
            .await;
        let prize = self
            .selftest_prize(id, server, username, &uuid, group)
            .await;
        let prize_report = self
            .selftest_step(
                id,
                "prize_summary",
                if prize.is_ok() {
                    "Rank step completed and absence verified."
                } else {
                    "Rank step FAILED; inspect rank reports. DM test continues."
                },
            )
            .await;
        let key = "notify_selftest";
        self.begin(
            id,
            key,
            "Inspect staff DM or banner-reviews before resending.",
        )
        .await?;
        let notification = self
            .deliver_notification(
                id,
                key,
                &user.to_string(),
                "Banner contest selftest: this is a real decision-notice notification test.",
                REVIEWS,
            )
            .await;
        self.selftest_step(
            id,
            "notification",
            match &notification {
                Ok("DM") => "Real staff DM delivered through the shared notification path.",
                Ok(_) => "Staff DMs closed: shared fallback delivered in banner-reviews.",
                Err(_) => "Notification failed; inspect journal before resending.",
            },
        )
        .await?;
        if notification.is_ok() {
            self.done(id, key, json!(true)).await?;
        }
        self.selftest_step(
            id,
            "summary",
            &format!(
                "Step summary: images={}, rank={}, notification={}.",
                images.is_ok(),
                prize.is_ok(),
                notification.is_ok()
            ),
        )
        .await?;
        images_report?;
        prize_report?;
        images?;
        prize?;
        notification?;
        Ok(())
    }
    async fn selftest_originals(&self, id: u64) -> Result<(Value, Vec<OriginalImage>)> {
        let path = format!("/guilds/{}", config::GUILD_ID);
        let before = self.request(reqwest::Method::GET, &path, None).await?;
        let mut originals = Vec::new();
        for field in ["banner", "splash", "discovery_splash"] {
            if let Some(hash) = before[field].as_str() {
                if hash.starts_with("a_") {
                    self.selftest_step(
                        id,
                        "images_skip",
                        "Animated current image: image step skipped without PATCH.",
                    )
                    .await?;
                    return Ok((before, Vec::new()));
                }
                // No size query: preserve the stored CDN representation, never a resized variant.
                let bytes = self
                    .download(&format!(
                        "https://cdn.discordapp.com/{}/{}/{hash}.png",
                        guild_cdn_field(field),
                        config::GUILD_ID
                    ))
                    .await?;
                originals.push(OriginalImage {
                    field,
                    dimensions: dimensions(&bytes)?,
                    bytes,
                });
            }
        }
        Ok((before, originals))
    }

    pub(super) async fn selftest_guild_images(&self, id: u64) -> Result<()> {
        let (before, originals) = self.selftest_originals(id).await?;
        if originals.is_empty() {
            return Ok(());
        }
        // Backup must be accepted and its message ID persisted before any guild change.
        self.begin(
            id,
            "image_backup",
            "Inspect banner-reviews backup before retrying.",
        )
        .await?;
        let backup = self.backup_images(&originals).await?;
        let backup_id = backup["id"].as_str().context("Backup message has no ID")?;
        let link = format!(
            "https://discord.com/channels/{}/{REVIEWS}/{backup_id}",
            config::GUILD_ID
        );
        self.done(
            id,
            "image_backup",
            json!({"message_id":backup_id,"link":link,"hashes":before}),
        )
        .await?;
        let Ok((encoded, restore)) = prepare_images(&originals).await else {
            self.selftest_step(id, "images_skip", &format!("Current image cannot safely exercise winner encoder; image step skipped without PATCH. Backup: {link}")).await?;
            return Ok(());
        };
        self.begin(
            id,
            "guild_images",
            &format!("Restore originals from {link} if interrupted."),
        )
        .await?;
        // Every error after the first PATCH (including journal/report/verification errors)
        // reaches restoration. A transport error may have applied the request remotely.
        let exercise = async {
            let (status, _) = self
                .modify_guild_images(encoded.clone(), "banner contest selftest")
                .await
                .map_err(|error| {
                    let status = error
                        .downcast_ref::<DiscordHttpError>()
                        .map_or_else(|| "unknown".to_owned(), |e| e.0.to_string());
                    anyhow::anyhow!("Modify Guild failed: HTTP {status}")
                })?;
            self.selftest_step(
                id,
                "guild_http",
                &format!("Single winner-path Modify Guild call: HTTP {status}."),
            )
            .await?;
            self.verify_images(&before, &originals, false).await?;
            self.done(id, "guild_images", json!(true)).await
        }
        .await;
        // Restoration is a safety action: attempt it even if saving its journal entry fails.
        let restore_journal = self
            .begin(
                id,
                "restore_images",
                &format!("Manual restore backup: {link}"),
            )
            .await;
        let restored = async {
            self.modify_guild_images(restore, "banner contest selftest restore")
                .await?;
            self.verify_images(&before, &originals, true).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if restored.is_err() {
            let warning = format!(
                "URGENT: ORIGINAL GUILD IMAGE RESTORE FAILED. Restore manually from backup: {link}"
            );
            tracing::error!("{warning}");
            let _ = self.selftest_step(id, "restore_failed", &warning).await;
            bail!("{warning}");
        }
        restore_journal?;
        self.done(id, "restore_images", json!(true)).await?;
        self.selftest_step(
            id,
            "restore_verified",
            "Original guild image hashes, bytes and dimensions restored and verified.",
        )
        .await?;
        if let Err(error) = &exercise {
            self.selftest_step(id, "guild_failure", &error.to_string())
                .await?;
        }
        exercise
    }

    async fn verify_images(
        &self,
        before: &Value,
        originals: &[OriginalImage],
        restored: bool,
    ) -> Result<()> {
        let after = self
            .request(
                reqwest::Method::GET,
                &format!("/guilds/{}", config::GUILD_ID),
                None,
            )
            .await?;
        for original in originals {
            let hash = after[original.field]
                .as_str()
                .context("Guild image missing after PATCH")?;
            let bytes = self
                .download(&format!(
                    "https://cdn.discordapp.com/{}/{}/{hash}.png",
                    guild_cdn_field(original.field),
                    config::GUILD_ID
                ))
                .await?;
            let (w, h) = dimensions(&bytes)?;
            if restored {
                if before[original.field] != after[original.field]
                    || (w, h) != original.dimensions
                    || bytes != original.bytes
                {
                    bail!("Original hash/bytes/dimensions were not restored");
                }
            } else if before[original.field] == after[original.field]
                || u64::from(w) * 9 != u64::from(h) * 16
            {
                bail!("Winner image hash/dimension verification failed");
            }
        }
        Ok(())
    }

    async fn backup_images(&self, originals: &[OriginalImage]) -> Result<Value> {
        let payload = json!({"content":"Banner selftest: backup before selftest. Original image bytes for manual restore.","allowed_mentions":{"parse":[]},"attachments":originals.iter().enumerate().map(|(i,o)| json!({"id":i,"filename":format!("{}.png",o.field)})).collect::<Vec<_>>()});
        // Only definite 429s are safe to retry. An ambiguous upload never permits PATCH.
        for attempt in 0..4 {
            let mut form =
                reqwest::multipart::Form::new().text("payload_json", payload.to_string());
            for (i, original) in originals.iter().enumerate() {
                form = form.part(
                    format!("files[{i}]"),
                    reqwest::multipart::Part::bytes(original.bytes.clone())
                        .file_name(format!("{}.png", original.field)),
                );
            }
            let response = self
                .http
                .post(format!("{}/channels/{REVIEWS}/messages", self.discord_api))
                .header("Authorization", format!("Bot {}", self.token))
                .multipart(form)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("Backup upload transport failure; no guild change"))?;
            if response.status().as_u16() == 429 && attempt < 3 {
                Self::rate_limit_wait(response).await?;
                continue;
            }
            return Self::response(response).await;
        }
        bail!("Backup upload rate limit exceeded")
    }

    async fn selftest_prize(
        &self,
        id: u64,
        server: &ServerService,
        username: &str,
        uuid: &str,
        group: &str,
    ) -> Result<()> {
        // Recheck after CDN work, immediately before any temporary rank command.
        if server.banner_uuid(username).await?.as_deref() != Some(uuid) {
            bail!("Username UUID changed before grant; no prize dispatched")
        }
        if server
            .ranks(username)
            .await?
            .context("Username not found before grant")?
            .iter()
            .any(|r| r.eq_ignore_ascii_case(group))
        {
            bail!("Requested prize appeared before grant; no prize dispatched")
        }
        self.begin(
            id,
            "prize",
            "Check ranks; selftest grant expires after one minute. Never repeat addtemp blindly.",
        )
        .await?;
        let grant_started = tokio::time::Instant::now();
        let granted = server
            .banner_prize_command(uuid, group, "addtemp", "1m")
            .await;
        // Always reach cleanup even when step reporting fails.
        let dispatch_report = self
            .selftest_step(
                id,
                "prize_grant",
                &format!(
                    "Winner-path addtemp 1m dispatched={}; checking /get-ranks for up to 30s.",
                    granted.is_ok()
                ),
            )
            .await;
        let verified = granted.is_ok() && server.verify_banner_prize(username, group).await;
        let verify_report = self
            .selftest_step(
                id,
                "prize_verify",
                &format!("Prize verified within 30s={verified}; subtracting only the test minute if present; otherwise waiting for expiry."),
            )
            .await;
        let removed = if verified {
            server
                .banner_prize_command(uuid, group, "removetemp", "1m")
                .await
        } else {
            // Dispatch acceptance does not establish execution order across proxies.
            tokio::time::sleep_until(grant_started + Duration::from_secs(70)).await;
            Ok(())
        };
        let remove_report = self
            .selftest_step(
                id,
                "prize_remove",
                &format!(
                    "removetemp 1m dispatched={}; otherwise waited for expiry; checking absence for up to 30s.",
                    verified && removed.is_ok()
                ),
            )
            .await;
        let gone = server
            .verify_banner_prize_presence(username, group, false)
            .await;
        self.selftest_step(id, "prize_result", &format!("Winner-path addtemp 1m dispatched={}, verified within 30s={verified}; removetemp dispatched={}, absence verified within 30s={gone}.", granted.is_ok(), verified && removed.is_ok())).await?;
        self.done(id, "prize", json!({"verified":verified,"gone":gone}))
            .await?;
        dispatch_report?;
        verify_report?;
        remove_report?;
        if !verified || removed.is_err() || !gone {
            bail!("Prize selftest failed; temporary test grant expires after one minute")
        }
        Ok(())
    }
}

async fn prepare_images(originals: &[OriginalImage]) -> Result<(Value, Value)> {
    let mut encoded = json!({});
    let mut restore = json!({});
    for original in originals {
        restore[original.field] = json!(original_uri(&original.bytes)?);
        let (w, h) = original.dimensions;
        if u64::from(w) * 9 != u64::from(h) * 16 {
            bail!("Current image is not 16:9");
        }
        let bytes = original.bytes.clone();
        let crop = tokio::task::spawn_blocking(move || image::crop(&bytes)).await??;
        encoded[original.field] =
            json!(format!("data:image/jpeg;base64,{}", STANDARD.encode(crop)));
    }
    Ok((encoded, restore))
}

fn guild_cdn_field(field: &str) -> &str {
    match field {
        "banner" => "banners",
        "splash" => "splashes",
        "discovery_splash" => "discovery-splashes",
        _ => unreachable!("only guild image fields are passed"),
    }
}
