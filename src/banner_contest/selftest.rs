use super::*;

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
        if !matches!(group, "primeultra" | "eliteultra" | "legend") {
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

        self.selftest_guild_images(id).await?;

        self.selftest_prize(id, server, username, &uuid, group)
            .await?;
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
                "Banner contest selftest: this is a real winner-path notification test.",
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
        notification?;
        self.done(id, key, json!(true)).await
    }
    async fn selftest_current_images(&self, id: u64) -> Result<(Value, Value)> {
        let path = format!("/guilds/{}", config::GUILD_ID);
        let before = self.request(reqwest::Method::GET, &path, None).await?;
        let mut images = json!({});
        for field in ["banner", "splash", "discovery_splash"] {
            if let Some(hash) = before[field].as_str() {
                // Animated images cannot be round-tripped through JPEG without visible change.
                if hash.starts_with("a_") {
                    bail!("Animated guild image cannot be safely self-tested")
                }
                let cdn_field = guild_cdn_field(field);
                let url = format!(
                    "https://cdn.discordapp.com/{cdn_field}/{}/{hash}.png?size=4096",
                    config::GUILD_ID
                );
                let bytes = self.download(&url).await?;
                let dimensions = ::image::ImageReader::new(std::io::Cursor::new(&bytes))
                    .with_guessed_format()?
                    .into_dimensions()?;
                if u64::from(dimensions.0) * 9 != u64::from(dimensions.1) * 16 {
                    bail!("Current guild image is not 16:9; refusing a visible crop")
                }
                let crop = image::crop(&bytes)?;
                images[field] = json!(format!("data:image/jpeg;base64,{}", STANDARD.encode(crop)));
                self.selftest_step(id, field, &format!("{field}: downloaded current CDN image at size 4096 and encoded 16:9 JPEG within winner limits.")).await?;
            } else {
                self.selftest_step(id, field, &format!("{field}: missing; skipped."))
                    .await?;
            }
        }
        Ok((before, images))
    }

    async fn selftest_guild_images(&self, id: u64) -> Result<()> {
        let path = format!("/guilds/{}", config::GUILD_ID);
        let (before, images) = self.selftest_current_images(id).await?;
        if images.as_object().is_some_and(|m| !m.is_empty()) {
            self.begin(
                id,
                "guild_images",
                "Inspect guild image hashes before repeating.",
            )
            .await?;
            match self.modify_guild_images(images.clone()).await {
                Ok((status, _)) => {
                    self.selftest_step(
                        id,
                        "guild_http",
                        &format!("Single winner-path Modify Guild call: HTTP {status}."),
                    )
                    .await?;
                }
                Err(error) => {
                    let status = error.downcast_ref::<DiscordHttpError>().map_or_else(
                        || "unknown (transport/response failure)".into(),
                        |e| e.0.to_string(),
                    );
                    self.selftest_step(
                        id,
                        "guild_http",
                        &format!("Modify Guild failed: HTTP {status}."),
                    )
                    .await?;
                    return Err(error);
                }
            }
            self.done(id, "guild_images", json!(true)).await?;
            let after = self.request(reqwest::Method::GET, &path, None).await?;
            for field in images.as_object().context("invalid images")?.keys() {
                let hash = after[field]
                    .as_str()
                    .context("guild image missing after PATCH")?;
                let cdn_field = guild_cdn_field(field);
                let bytes = self
                    .download(&format!(
                        "https://cdn.discordapp.com/{cdn_field}/{}/{hash}.png?size=4096",
                        config::GUILD_ID
                    ))
                    .await?;
                let (w, h) = ::image::ImageReader::new(std::io::Cursor::new(bytes))
                    .with_guessed_format()?
                    .into_dimensions()?;
                let changed = before[field] != after[field];
                self.selftest_step(
                    id,
                    &format!("verify_{field}"),
                    &format!(
                        "{field}: hash changed={changed}, dimensions={w}x{h}, 16:9={}",
                        u64::from(w) * 9 == u64::from(h) * 16
                    ),
                )
                .await?;
                if !changed || u64::from(w) * 9 != u64::from(h) * 16 {
                    bail!("guild image verification failed")
                }
            }
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
                &format!("Prize verified within 30s={verified}; removing test temporary parent."),
            )
            .await;
        let removed = server
            .banner_prize_command(uuid, group, "removetemp", "")
            .await;
        let remove_report = self
            .selftest_step(
                id,
                "prize_remove",
                &format!(
                    "removetemp dispatched={}; checking absence for up to 30s.",
                    removed.is_ok()
                ),
            )
            .await;
        let gone = server
            .verify_banner_prize_presence(username, group, false)
            .await;
        self.selftest_step(id, "prize_result", &format!("Winner-path addtemp 1m dispatched={}, verified within 30s={verified}; removetemp dispatched={}, absence verified within 30s={gone}.", granted.is_ok(), removed.is_ok())).await?;
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
