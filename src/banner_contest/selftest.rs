use super::*;
use chrono::Timelike as _;

pub(super) const TEST_PLAYER: &str = "BannerSelftest";

fn dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    Ok(::image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()?)
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
                    format!("FAILED: {detail}. No automatic re-dispatch of prize commands.")
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
        server.selftest_account_safety(username, &uuid).await?;
        let ranks = server
            .selftest_ranks(username, &uuid)
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
                    "Offline image checks completed or safely skipped; no guild PATCH."
                } else {
                    "Offline image checks FAILED; inspect reports. Rank and DM tests continue."
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
    pub(super) async fn selftest_guild_images(&self, id: u64) -> Result<()> {
        let guild = self
            .request(
                reqwest::Method::GET,
                &format!("/guilds/{}", config::GUILD_ID),
                None,
            )
            .await?;
        let mut failed = false;
        for field in ["banner", "splash", "discovery_splash"] {
            let Some(hash) = guild[field].as_str() else {
                self.selftest_step(
                    id,
                    &format!("image_{field}"),
                    &format!("{field}: absent; offline check skipped."),
                )
                .await?;
                continue;
            };
            // PNG is a CDN representation, not original upload bytes. Never re-upload it.
            let check = async {
                let bytes = self.download(&format!("https://cdn.discordapp.com/{}/{}/{hash}.png", guild_cdn_field(field), config::GUILD_ID)).await?;
                let encoded = tokio::task::spawn_blocking(move || image::crop(&bytes)).await??;
                let (w, h) = dimensions(&encoded)?;
                let valid = ::image::guess_format(&encoded)? == ::image::ImageFormat::Jpeg
                    && u64::from(w) * 9 == u64::from(h) * 16
                    && (field != "banner" || (w >= 960 && h >= 540));
                Ok::<_, anyhow::Error>(format!("{field}: winner JPEG {} bytes, {w}x{h}; documented PNG/JPEG, 16:9 and banner >=960x540 requirements satisfied={valid}; conservative <3 MiB encoder limit satisfied={}. Offline only; feature eligibility and HTTP acceptance untested.", encoded.len(), encoded.len() < image::MAX_GUILD_IMAGE))
            }.await;
            let report = if let Ok(report) = check {
                report
            } else {
                failed = true;
                format!("{field}: offline download/encoder check failed; no guild change.")
            };
            self.selftest_step(id, &format!("image_{field}"), &report)
                .await?;
        }
        if failed {
            bail!("Offline image check failed; no guild images changed");
        }
        Ok(())
    }

    async fn selftest_prize(
        &self,
        id: u64,
        server: &ServerService,
        username: &str,
        uuid: &str,
        group: &str,
    ) -> Result<()> {
        server.selftest_account_safety(username, uuid).await?;
        // Recheck after CDN work, immediately before any temporary rank command.
        if server.banner_uuid(username).await?.as_deref() != Some(uuid) {
            bail!("Username UUID changed before grant; no prize dispatched")
        }
        if server
            .selftest_ranks(username, uuid)
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
        let verified = granted.is_ok()
            && server
                .verify_banner_prize_presence_checked(username, group, true, Some(uuid))
                .await;
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
            .verify_banner_prize_presence_checked(username, group, false, Some(uuid))
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

fn guild_cdn_field(field: &str) -> &str {
    match field {
        "banner" => "banners",
        "splash" => "splashes",
        "discovery_splash" => "discovery-splashes",
        _ => unreachable!("only guild image fields are passed"),
    }
}
