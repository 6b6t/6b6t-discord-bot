use super::{image::crop, model::*};
use chrono::{DateTime, TimeZone as _, Timelike as _, Utc};
use chrono_tz::Europe::Warsaw;
use serde_json::json;
use std::io::Cursor;

fn contest() -> Contest {
    let s = Schedule::monthly(2026, 11).unwrap();
    Contest {
        id: 1,
        year: 2026,
        month: 11,
        state: "open".into(),
        call_at: s.call,
        close_at: s.close,
        voting_at: s.voting,
        end_at: s.end,
        dry_run: false,
        reminded: false,
        theme: None,
        call_message_id: None,
    }
}
fn entry(id: u64, votes: u64, submitted: i64) -> Entry {
    Entry {
        id,
        contest_id: 1,
        discord_id: "123".into(),
        username: "player".into(),
        uuid: "00000000-0000-0000-0000-000000000001".into(),
        email: "private@example.org".into(),
        status: "approved".into(),
        decider: None,
        reason: None,
        submitted_at: submitted,
        shuffle_key: id.to_string(),
        review_message_id: None,
        vote_message_id: None,
        votes,
        revision: 0,
        review_revision: 0,
        review_closed: false,
    }
}
#[test]
fn calendar_handles_every_month_length_and_leap_year() {
    for (year, month, call) in [
        (2026, 11, "2026-10-25"),
        (2026, 12, "2026-11-24"),
        (2027, 3, "2027-02-22"),
        (2028, 3, "2028-02-23"),
        (2026, 5, "2026-04-24"),
    ] {
        let s = Schedule::monthly(year, month).unwrap();
        assert_eq!(
            DateTime::from_timestamp(s.call, 0)
                .unwrap()
                .with_timezone(&Warsaw)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            format!("{call} 22:00")
        );
        for (time, hour) in [(s.call, 22), (s.close, 22), (s.voting, 10), (s.end, 10)] {
            assert_eq!(
                DateTime::from_timestamp(time, 0)
                    .unwrap()
                    .with_timezone(&Warsaw)
                    .hour(),
                hour
            );
        }
        assert!(s.call < s.close && s.close < s.voting && s.voting < s.end);
    }
}
#[test]
fn schedule_keeps_local_times_across_both_dst_changes() {
    let spring = Schedule::monthly(2026, 4).unwrap();
    let autumn = Schedule::monthly(2026, 11).unwrap();
    // Spring skips an hour between 25 March call and 1 April end.
    assert_eq!(spring.end - spring.call, 6 * 86400 + 11 * 3600);
    assert_eq!(spring.close - spring.call, 72 * 3600);
    assert_eq!(autumn.end - autumn.call, 6 * 86400 + 12 * 3600);
    // One-off ending 29 October crosses the autumn change after its call.
    let one_off = Schedule::ending(chrono::NaiveDate::from_ymd_opt(2026, 10, 29).unwrap()).unwrap();
    assert_eq!(one_off.end - one_off.call, 6 * 86400 + 13 * 3600);
}
#[test]
fn theme_reminder_keeps_warsaw_time_across_autumn_dst() {
    let call = Schedule::monthly(2026, 11).unwrap().call;
    let reminder = DateTime::from_timestamp(theme_reminder_at(call).unwrap(), 0)
        .unwrap()
        .with_timezone(&Warsaw);
    assert_eq!(reminder.format("%F %H:%M").to_string(), "2026-10-23 22:00");
    assert_eq!(call - reminder.timestamp(), 49 * 3600);
}

#[test]
fn one_off_and_minute_schedules() {
    let longer = Schedule::test_with(1000, 10);
    assert_eq!(
        (longer.close, longer.voting, longer.end),
        (1600, 2200, 2800)
    );
    assert_eq!(Schedule::test_with(0, 99).end, 30 * 60 * 3);
    let s = Schedule::test(1000);
    assert_eq!((s.call, s.close, s.voting, s.end), (1000, 1060, 1120, 1180));
    assert!(Schedule::monthly(2026, 0).is_err());
    assert!(Schedule::monthly(2026, 13).is_err());
    let s = Schedule::ending(chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()).unwrap();
    assert_eq!(
        DateTime::from_timestamp(s.voting, 0)
            .unwrap()
            .with_timezone(&Warsaw)
            .format("%F %H:%M")
            .to_string(),
        "2026-10-17 10:00"
    );
}
#[test]
fn eligibility_uses_all_ranks_and_rejects_upgraded_groups() {
    for (groups, expected) in [
        (vec!["prime"], Some("primeultra")),
        (vec!["prime+"], Some("primeultra")),
        (vec!["elite"], Some("eliteultra")),
        (vec!["elite+", "prime"], Some("eliteultra")),
        (vec!["apex", "elite+"], Some("legend")),
        (vec!["apex", "primeultra"], Some("legend")),
        (vec![], None),
        (vec!["default"], None),
        (vec!["prime", "primeultra"], None),
        (vec!["elite+", "eliteultra"], None),
        (vec!["apex", "legend"], None),
        (vec!["prime", "legendultra"], None),
    ] {
        assert_eq!(
            prize(&groups.into_iter().map(str::to_owned).collect::<Vec<_>>()),
            expected
        );
    }
}
#[test]
fn docs_shaped_file_upload_modal_resolves_attachments() {
    // https://docs.discord.com/developers/components/reference#file-upload
    let data = json!({"custom_id":"banner:form:1","components":[
        {"type":18,"id":1,"component":{"type":4,"id":2,"custom_id":"username","value":"Steve"}},
        {"type":18,"id":3,"component":{"type":4,"id":4,"custom_id":"email","value":"steve@example.org"}},
        {"type":18,"id":5,"component":{"type":19,"id":6,"custom_id":"image","values":["111111111111111111111"]}}
    ],"resolved":{"attachments":{"111111111111111111111":{"id":"111111111111111111111","content_type":"image/png","ephemeral":true,"filename":"bug.png","height":604,"width":2482,"placeholder":"/PcBAoBQydvKesabEIoMsdg=","placeholder_version":1,"size":241_394,"url":"https://cdn.discordapp.com/ephemeral-attachments/2222222222222222222/111111111111111111111/bug.png?ex=68dc7ce1&is=68db2b61&hm=5954f90117ccf8716ffa6c7f97a778a0d039810c9584045f400d8a9fff590768&","proxy_url":"https://media.discordapp.net/ephemeral-attachments/2222222222222222222/111111111111111111111/bug.png?ex=68dc7ce1&is=68db2b61&hm=5954f90117ccf8716ffa6c7f97a778a0d039810c9584045f400d8a9fff590768&"}}}});
    let a = parse_application(&data).unwrap();
    assert_eq!(a.username, "Steve");
    assert_eq!(a.email, "steve@example.org");
    assert_eq!(a.size, 241_394);
    let mut missing = data.clone();
    missing["resolved"] = json!({});
    assert!(parse_application(&missing).is_err());
    let mut two = data.clone();
    two["components"][2]["component"]["values"] = json!(["111111111111111111111", "222"]);
    assert!(parse_application(&two).is_err());
    for url in [
        "http://cdn.discordapp.com/attachments/x/y/z",
        "https://example.org/image.png",
        "https://cdn.discordapp.com.evil.org/attachments/x/y/z",
        "https://cdn.discordapp.com:444/attachments/x/y/z",
        "https://cdn.discordapp.com/not-attachments/foo",
    ] {
        let mut bad = data.clone();
        bad["resolved"]["attachments"]["111111111111111111111"]["url"] = json!(url);
        assert!(parse_application(&bad).is_err());
    }
}
#[test]
fn modal_and_review_payloads_are_anonymous_and_required() {
    let modal = apply_modal(42);
    let upload = &modal["data"]["components"][2]["component"];
    assert_eq!(upload["type"], 19);
    assert_eq!(upload["file_types"], json!(["image"]));
    assert_eq!(upload["min_values"], 1);
    assert_eq!(upload["max_values"], 1);
    assert_eq!(upload["required"], true);
    let buttons = review_components(42, true);
    assert_eq!(buttons[0]["components"][0]["disabled"], true);
    let text = buttons.to_string();
    for private in ["username", "discord_id", "email", "prize"] {
        assert!(!text.contains(private));
    }
}
#[test]
fn usernames_and_email_are_validated_without_identity_leaks() {
    for email in [
        "test@example.org",
        "a+b@example.co.uk",
        "first.last@example.org",
    ] {
        assert!(valid_email(email));
    }
    for email in [
        "test",
        "a@b",
        "a@@example.org",
        "@example.org",
        "a b@example.org",
        ".a@example.org",
        "a..b@example.org",
        "a@example..org",
        "a@-example.org",
    ] {
        assert!(!valid_email(email));
    }
    assert!(valid_username("player_123"));
    for name in ["ab", "a b", "@everyone", "name; op player"] {
        assert!(!valid_username(name));
    }
    assert!(!winner_text(&contest(), &entry(1, 2, 0), "legend").contains("private@example.org"));
}
#[test]
fn winner_tie_uses_submission_time_then_id() {
    let mut entries = [
        entry(3, 7, 20),
        entry(2, 7, 10),
        entry(1, 7, 10),
        entry(4, 1, 0),
    ];
    entries.sort_by(winner_order);
    assert_eq!(entries.map(|e| e.id), [1, 2, 3, 4]);
}
#[test]
fn deadlines_reject_changes_and_test_channels_stay_private() {
    let mut c = contest();
    assert!(can_review(&c, c.voting_at - 1));
    assert!(!can_review(&c, c.voting_at));
    c.state = "voting".into();
    assert!(!can_review(&c, c.voting_at - 1));
    c.dry_run = true;
    assert_eq!(c.channel(), super::REVIEWS);
    c.dry_run = false;
    assert_eq!(c.channel(), super::ANNOUNCEMENTS);
    assert!(reviewer(
        &json!({"member":{"roles":["1533970494284365854"]}})
    ));
    assert!(!reviewer(&json!({"member":{"roles":[],"permissions":"8"}})));
}
#[test]
fn jpeg_crop_is_exactly_16_by_9_and_rejects_bad_images() {
    for (w, h) in [(1280, 720), (2048, 1536), (3840, 2160)] {
        let img = image::DynamicImage::new_rgb8(w, h);
        let mut bytes = Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let output = crop(bytes.get_ref()).unwrap();
        assert_eq!(
            image::guess_format(&output).unwrap(),
            image::ImageFormat::Jpeg
        );
        let decoded = image::load_from_memory(&output).unwrap();
        assert_eq!(decoded.width() * 9, decoded.height() * 16);
        assert!(decoded.width() <= 1920 && decoded.height() <= 1080);
        assert!(output.len() < super::image::MAX_GUILD_IMAGE);
    }
    assert!(crop(b"not an image").is_err());
    let img = image::DynamicImage::new_rgb8(1279, 720);
    let mut bytes = Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    assert!(crop(bytes.get_ref()).is_err());
}
#[test]
fn expiry_matches_luckperms_duration_parser() {
    let now = Utc.with_ymd_and_hms(2027, 1, 31, 10, 0, 0).unwrap();
    assert_eq!(
        month_expiry(now).unwrap(),
        now + chrono::Duration::seconds(2_629_746)
    );
}

// The local HTTP fixture and sequential integration scenario intentionally share
// one isolated database to exercise restarts without parallel-test interference.
#[allow(clippy::too_many_lines)]
pub(crate) mod local_integration {
    use super::*;
    use anyhow::{Context as _, Result};
    use base64::Engine as _;
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
        sync::Mutex,
    };

    type FailureRules = Arc<Mutex<Vec<(String, String, usize, u16)>>>;
    pub(crate) struct Mock {
        base: String,
        requests: Arc<Mutex<Vec<(String, String, Value)>>>,
        awarded: Arc<AtomicBool>,
        fail_image: Arc<AtomicBool>,
        failures: FailureRules,
        ranks: Arc<Mutex<std::collections::HashMap<String, Vec<String>>>>,
        verify_delay: Arc<AtomicU64>,
        deny_command: Arc<AtomicBool>,
        created_posts: Arc<AtomicU64>,
        guild: Arc<Mutex<Value>>,
        attachment: Arc<Mutex<Vec<u8>>>,
        uploads: Arc<Mutex<Vec<Vec<u8>>>>,
        audit_reasons: Arc<Mutex<Vec<String>>>,
        dm_open: Arc<AtomicBool>,
        pub(crate) gateway_remaining: Arc<AtomicU64>,
        pub(crate) gateway_url: Arc<Mutex<String>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Mock {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = requests.clone();
            let awarded = Arc::new(AtomicBool::new(false));
            let grant = awarded.clone();
            let fail_image = Arc::new(AtomicBool::new(false));
            let failure = fail_image.clone();
            let failures = Arc::new(Mutex::new(Vec::<(String, String, usize, u16)>::new()));
            let failing = failures.clone();
            let ranks = Arc::new(Mutex::new(
                std::collections::HashMap::<String, Vec<String>>::new(),
            ));
            let rank_map = ranks.clone();
            let verify_delay = Arc::new(AtomicU64::new(0));
            let delay = verify_delay.clone();
            let deny_command = Arc::new(AtomicBool::new(false));
            let denied = deny_command.clone();
            let dm_open = Arc::new(AtomicBool::new(false));
            let open_dm = dm_open.clone();
            let guild = Arc::new(Mutex::new(json!({})));
            let guild_state = guild.clone();
            let initial_guild = Arc::new(Mutex::new(None::<Value>));
            let uploads = Arc::new(Mutex::new(Vec::new()));
            let upload_log = uploads.clone();
            let audit_reasons = Arc::new(Mutex::new(Vec::new()));
            let audit_log = audit_reasons.clone();
            let gateway_remaining = Arc::new(AtomicU64::new(99));
            let remaining = gateway_remaining.clone();
            let gateway_url = Arc::new(Mutex::new("ws://127.0.0.1:9".to_owned()));
            let gateway_address = gateway_url.clone();
            let created_posts = Arc::new(AtomicU64::new(0));
            let created = created_posts.clone();
            let nonces = Arc::new(Mutex::new(
                std::collections::HashMap::<String, String>::new(),
            ));
            let messages = Arc::new(AtomicU64::new(1000));
            let mut attachment = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(1280, 720)
                .write_to(&mut attachment, image::ImageFormat::Png)
                .unwrap();
            let attachment = Arc::new(Mutex::new(attachment.into_inner()));
            let download_bytes = attachment.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        break;
                    };
                    let recorded = recorded.clone();
                    let grant = grant.clone();
                    let failure = failure.clone();
                    let messages = messages.clone();
                    let attachment = download_bytes.clone();
                    let initial_guild = initial_guild.clone();
                    let upload_log = upload_log.clone();
                    let audit_log = audit_log.clone();
                    let failing = failing.clone();
                    let rank_map = rank_map.clone();
                    let delay = delay.clone();
                    let denied = denied.clone();
                    let remaining = remaining.clone();
                    let gateway_address = gateway_address.clone();
                    let guild_state = guild_state.clone();
                    let open_dm = open_dm.clone();
                    let created = created.clone();
                    let nonces = nonces.clone();
                    tokio::spawn(async move {
                        let mut bytes = Vec::new();
                        let mut buffer = [0_u8; 8192];
                        let (header_end, body_size) = loop {
                            let count = socket.read(&mut buffer).await.unwrap();
                            if count == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&buffer[..count]);
                            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                                let headers = String::from_utf8_lossy(&bytes[..end]);
                                let size = headers
                                    .lines()
                                    .find_map(|line| {
                                        line.to_ascii_lowercase()
                                            .strip_prefix("content-length:")
                                            .map(|n| n.trim().parse::<usize>().unwrap())
                                    })
                                    .unwrap_or(0);
                                break (end + 4, size);
                            }
                        };
                        while bytes.len() < header_end + body_size {
                            let count = socket.read(&mut buffer).await.unwrap();
                            if count == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&buffer[..count]);
                        }
                        let header = String::from_utf8_lossy(&bytes[..header_end]);
                        let mut first = header.lines().next().unwrap().split_whitespace();
                        let method = first.next().unwrap().to_owned();
                        let path = first.next().unwrap().to_owned();
                        let body = &bytes[header_end..header_end + body_size];
                        if header.to_ascii_lowercase().contains("multipart/form-data") {
                            upload_log.lock().await.push(body.to_vec());
                        }
                        if method == "PATCH" && path.starts_with("/guilds/") {
                            audit_log.lock().await.push(
                                header
                                    .lines()
                                    .find_map(|line| {
                                        line.to_ascii_lowercase()
                                            .strip_prefix("x-audit-log-reason:")
                                            .map(|v| v.trim().to_owned())
                                    })
                                    .unwrap_or_default(),
                            );
                        }
                        let payload = serde_json::from_slice::<Value>(body).unwrap_or_else(|_| {
                            let multipart = String::from_utf8_lossy(body);
                            multipart
                                .find("name=\"payload_json\"")
                                .and_then(|start| {
                                    multipart[start..]
                                        .find("\r\n\r\n")
                                        .map(|offset| start + offset + 4)
                                })
                                .and_then(|start| {
                                    multipart[start..]
                                        .find("\r\n--")
                                        .map(|end| &multipart[start..start + end])
                                })
                                .and_then(|text| serde_json::from_str(text).ok())
                                .unwrap_or(Value::Null)
                        });
                        recorded
                            .lock()
                            .await
                            .push((method.clone(), path.clone(), payload.clone()));
                        let injected = {
                            let mut failures = failing.lock().await;
                            failures
                                .iter_mut()
                                .find(|(m, p, n, _)| {
                                    *m == method && path.starts_with(p.as_str()) && *n > 0
                                })
                                .map(|(_, _, n, status)| {
                                    *n -= 1;
                                    *status
                                })
                        };
                        let (status, response) = if injected == Some(599) {
                            // Discord accepted the public post but its response was lost to a 5xx.
                            let id = messages.fetch_add(1, Ordering::SeqCst).to_string();
                            if let Some(nonce) = payload["nonce"].as_str() {
                                nonces.lock().await.insert(nonce.to_owned(), id);
                            }
                            created.fetch_add(1, Ordering::SeqCst);
                            (500, b"{}".to_vec())
                        } else if let Some(status) = injected {
                            (status, b"{}".to_vec())
                        } else if path == "/gateway/bot" {
                            (200,json!({"url":*gateway_address.lock().await, "session_start_limit":{"remaining":remaining.load(Ordering::SeqCst)}}).to_string().into_bytes())
                        } else if path == "/attachment" {
                            (200, attachment.lock().await.clone())
                        } else if path == "/get-ranks" {
                            let username = payload["username"]
                                .as_str()
                                .or_else(|| payload["player"].as_str())
                                .unwrap_or_default();
                            let mapped = rank_map.lock().await.get(username).cloned();
                            let delayed = delay
                                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                                    n.checked_sub(1)
                                })
                                .is_ok();
                            let groups = mapped.unwrap_or_else(|| {
                                if grant.load(Ordering::SeqCst) && !delayed {
                                    vec!["prime".into(), "primeultra".into()]
                                } else {
                                    vec!["prime".into()]
                                }
                            });
                            (
                                200,
                                json!({"success":true,"ranks":groups})
                                    .to_string()
                                    .into_bytes(),
                            )
                        } else if path == "/run-command" || path == "/proxy/run-command" {
                            if denied.load(Ordering::SeqCst) {
                                (403, b"{}".to_vec())
                            } else {
                                grant.store(
                                    !payload["command"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .contains("removetemp"),
                                    Ordering::SeqCst,
                                );
                                (200, json!({"success":true}).to_string().into_bytes())
                            }
                        } else if path.starts_with("/guilds/")
                            && method == "PATCH"
                            && failure.load(Ordering::SeqCst)
                        {
                            (403, b"{}".to_vec())
                        } else if path.starts_with("/guilds/") {
                            let mut guild = guild_state.lock().await;
                            if method == "PATCH" {
                                let mut initial = initial_guild.lock().await;
                                let initial = initial.get_or_insert_with(|| guild.clone());
                                for field in ["banner", "splash", "discovery_splash"] {
                                    if !payload[field].is_null() {
                                        guild[field] = if payload[field]
                                            .as_str()
                                            .is_some_and(|s| s.starts_with("data:image/png;"))
                                        {
                                            initial[field].clone()
                                        } else {
                                            json!(format!("changed_{field}"))
                                        };
                                    }
                                }
                            }
                            (200, guild.to_string().into_bytes())
                        } else if path.contains("/reactions/") && method == "GET" {
                            let users = if path.contains("type=1") {
                                vec![]
                            } else if path.contains("after=") {
                                vec![
                                    json!({"id":"101","bot":false}),
                                    json!({"id":"102","bot":true}),
                                ]
                            } else {
                                (1..=100)
                                    .map(|id| json!({"id":id.to_string(),"bot":id==1}))
                                    .collect()
                            };
                            (200, serde_json::to_vec(&users).unwrap())
                        } else if path == "/users/@me/channels" {
                            if open_dm.load(Ordering::SeqCst) {
                                (200, br#"{"id":"9000"}"#.to_vec())
                            } else {
                                (403, b"{}".to_vec())
                            }
                        } else if path.contains("/messages") && method == "POST" {
                            let nonce = if payload["enforce_nonce"] == true {
                                payload["nonce"].as_str()
                            } else {
                                None
                            };
                            let mut nonces = nonces.lock().await;
                            let existing = nonce.and_then(|n| nonces.get(n)).cloned();
                            let id = existing.unwrap_or_else(|| {
                                created.fetch_add(1, Ordering::SeqCst);
                                let id = messages.fetch_add(1, Ordering::SeqCst).to_string();
                                if let Some(nonce) = nonce {
                                    nonces.insert(nonce.to_owned(), id.clone());
                                }
                                id
                            });
                            (200, json!({"id":id}).to_string().into_bytes())
                        } else {
                            (200, b"{}".to_vec())
                        };
                        let header = format!(
                            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            response.len()
                        );
                        socket.write_all(header.as_bytes()).await.unwrap();
                        socket.write_all(&response).await.unwrap();
                    });
                }
            });
            Self {
                base,
                requests,
                awarded,
                fail_image,
                failures,
                ranks,
                verify_delay,
                deny_command,
                created_posts,
                guild,
                attachment,
                uploads,
                audit_reasons,
                dm_open,
                gateway_remaining,
                gateway_url,
                task,
            }
        }
    }
    fn interaction(custom: &str, kind: u64) -> Value {
        json!({"id":"555","application_id":"666","token":"local-fake-token","type":kind,"guild_id":"917520262797344779","member":{"user":{"id":"123"},"roles":["917520262939938915"]},"data":{"custom_id":custom}})
    }
    async fn insert_entry(
        service: &super::super::BannerService,
        contest: u64,
        user: &str,
        name: &str,
        uuid: &str,
        time: i64,
    ) -> Result<u64> {
        let mut source = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1280, 720).write_to(&mut source, image::ImageFormat::Png)?;
        Ok(sqlx::query("INSERT INTO banner_submissions(contest_id,discord_id,username,uuid,email,prize,image,submitted_at,shuffle_key) VALUES(?,?,?,?,?,'primeultra',?,?,?)").bind(contest).bind(user).bind(name).bind(uuid).bind("private@example.org").bind(crop(source.get_ref())?).bind(time).bind(uuid::Uuid::new_v4().to_string()).execute(&service.pool).await?.last_insert_id())
    }

    pub(crate) async fn fixture(
        name: &str,
    ) -> Result<(
        super::super::BannerService,
        crate::server::ServerService,
        Mock,
    )> {
        crate::install_crypto_provider()?;
        let url = std::env::var("BANNER_TEST_DATABASE_URL").context("set local test DB URL")?;
        let mut parsed = reqwest::Url::parse(&url)?;
        assert!(
            matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
                && parsed.path().starts_with("/banner_test")
        );
        let database = format!("banner_test_fix_{name}");
        parsed.set_path("/mysql");
        let admin = sqlx::MySqlPool::connect(parsed.as_str()).await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS `{database}`"
        )))
        .execute(&admin)
        .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE `{database}`")))
            .execute(&admin)
            .await?;
        admin.close().await;
        parsed.set_path(&format!("/{database}"));
        let pool = sqlx::MySqlPool::connect(parsed.as_str()).await?;
        super::super::ensure_schema(&pool).await?;
        sqlx::query("CREATE TABLE player_info(uuid CHAR(36) PRIMARY KEY,name VARCHAR(16),first_join BIGINT)").execute(&pool).await?;
        sqlx::query(
            "INSERT INTO player_info VALUES('00000000-0000-0000-0000-000000000001','Steve',0)",
        )
        .execute(&pool)
        .await?;
        let mock = Mock::start().await;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()?;
        let env = Arc::new(crate::config::Environment {
            discord_token: "local-fake-token".into(),
            proxy_command_base_url: Some(format!("{}/proxy", mock.base)),
            proxy_command_access_token: Some("fake".into()),
            rank_service_base_url: Some(mock.base.clone()),
            rank_service_access_token: Some("fake".into()),
            ..Default::default()
        });
        let mut service = super::super::BannerService::new(pool.clone(), http.clone(), &env);
        service.discord_api = mock.base.clone();
        let server = crate::server::ServerService::new(
            http,
            env,
            Some(crate::database::Databases {
                link: pool.clone(),
                stats: pool,
            }),
        );
        Ok((service, server, mock))
    }
    async fn opened(service: &super::super::BannerService, dry: bool) -> Result<(u64, u64)> {
        let now = Utc::now().timestamp();
        let id = service
            .insert_contest(
                2030,
                1,
                Schedule {
                    call: now - 1,
                    close: now + 86400,
                    voting: now + 129_600,
                    end: now + 388_800,
                },
                dry,
            )
            .await?;
        service.call(&service.contest(id).await?).await?;
        let entry = insert_entry(
            service,
            id,
            "123",
            "Steve",
            "00000000-0000-0000-0000-000000000001",
            now,
        )
        .await?;
        Ok((id, entry))
    }
    async fn retry_now(service: &super::super::BannerService, id: u64) -> Result<()> {
        let mut effects: Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>("SELECT effects FROM banner_contests WHERE id=?")
                .bind(id)
                .fetch_one(&service.pool)
                .await?,
        )?;
        for effect in effects.as_object_mut().unwrap().values_mut() {
            effect["retry_at"] = json!(0);
        }
        sqlx::query("UPDATE banner_contests SET effects=? WHERE id=?")
            .bind(effects.to_string())
            .bind(id)
            .execute(&service.pool)
            .await?;
        Ok(())
    }
    fn review_path() -> String {
        format!("/channels/{}/messages", super::super::REVIEWS)
    }

    async fn selftest_fixture(
        name: &str,
    ) -> Result<(
        super::super::BannerService,
        crate::server::ServerService,
        Mock,
    )> {
        let (service, server, mock) = fixture(name).await?;
        sqlx::query("INSERT INTO player_info VALUES('00000000-0000-0000-0000-000000000002','BannerSelftest',0)").execute(&service.pool).await?;
        Ok((service, server, mock))
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn selftest_real_paths_cleanup_and_private_fallback() -> Result<()> {
        let (service, server, mock) = selftest_fixture("selftest").await?;
        *mock.guild.lock().await =
            json!({"banner":"old_banner","splash":"old_splash","discovery_splash":"old_discovery"});
        service
            .selftest(&server, 42, "BannerSelftest", "primeultra")
            .await?;
        assert!(!mock.awarded.load(Ordering::SeqCst));
        let requests = mock.requests.lock().await;
        let patches: Vec<_> = requests
            .iter()
            .filter(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
            .collect();
        assert_eq!(patches.len(), 2);
        let original_bytes = mock.attachment.lock().await.clone();
        for field in ["banner", "splash", "discovery_splash"] {
            assert_eq!(
                patches[1].2[field],
                json!(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&original_bytes)
                ))
            );
        }
        let backup_index = requests
            .iter()
            .position(|(_, p, v)| {
                p == &review_path()
                    && v["content"]
                        .as_str()
                        .is_some_and(|s| s.contains("backup before selftest"))
            })
            .unwrap();
        let patch_index = requests
            .iter()
            .position(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
            .unwrap();
        assert!(backup_index < patch_index);
        assert_eq!(
            requests[backup_index].2["attachments"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(mock.uploads.lock().await.iter().any(|body| {
            body.windows(original_bytes.len())
                .filter(|window| *window == original_bytes)
                .count()
                == 3
        }));
        assert_eq!(
            *mock.guild.lock().await,
            json!({"banner":"old_banner","splash":"old_splash","discovery_splash":"old_discovery"})
        );
        assert_eq!(
            *mock.audit_reasons.lock().await,
            vec!["banner contest selftest", "banner contest selftest restore"]
        );
        let row: String = sqlx::query_scalar(
            "SELECT effects FROM banner_contests WHERE contest_key LIKE 'selftest-%'",
        )
        .fetch_one(&service.pool)
        .await?;
        assert!(
            serde_json::from_str::<Value>(&row)?["image_backup"]["result"]["message_id"]
                .is_string()
        );

        for field in ["banner", "splash", "discovery_splash"] {
            use base64::Engine as _;
            let uri = patches[0].2[field].as_str().unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(uri.strip_prefix("data:image/jpeg;base64,").unwrap())?;
            let image = image::load_from_memory(&bytes)?;
            assert_eq!(image.width() * 9, image.height() * 16);
            assert!(bytes.len() < super::super::image::MAX_GUILD_IMAGE);
        }
        let commands: Vec<_> = requests
            .iter()
            .filter(|(_, p, _)| p.ends_with("run-command"))
            .map(|(_, _, v)| v["command"].as_str().unwrap())
            .collect();
        assert_eq!(
            commands,
            vec![
                "lpv user 00000000-0000-0000-0000-000000000002 parent addtemp primeultra 1m",
                "lpv user 00000000-0000-0000-0000-000000000002 parent removetemp primeultra 1m"
            ]
        );
        assert!(requests.iter().any(|(_, p, v)| p == &review_path()
            && v["content"].as_str().is_some_and(|t| t.contains("<@42>"))));
        assert!(
            !requests
                .iter()
                .any(|(_, p, _)| p.contains(&super::super::GENERAL.to_string())
                    || p.contains(&super::super::ANNOUNCEMENTS.to_string()))
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn selftest_skips_missing_images_and_refuses_existing_prize() -> Result<()> {
        let (service, server, mock) = selftest_fixture("selftest_safe").await?;
        mock.ranks
            .lock()
            .await
            .insert("BannerSelftest".into(), vec!["primeultra".into()]);
        assert!(
            service
                .selftest(&server, 42, "BannerSelftest", "primeultra")
                .await
                .is_err()
        );
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(_, p, _)| p.ends_with("run-command"))
        );
        mock.ranks.lock().await.clear();
        service
            .selftest(&server, 42, "BannerSelftest", "primeultra")
            .await?;
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn selftest_real_dm_and_failed_grant_cleanup() -> Result<()> {
        let (service, server, mock) = selftest_fixture("selftest_dm").await?;
        mock.dm_open.store(true, Ordering::SeqCst);
        service
            .selftest(&server, 42, "BannerSelftest", "primeultra")
            .await?;
        assert!(mock.requests.lock().await.iter().any(|(_, p, v)| {
            p == "/channels/9000/messages"
                && v["content"]
                    .as_str()
                    .is_some_and(|t| t.contains("decision-notice"))
        }));
        mock.requests.lock().await.clear();
        mock.deny_command.store(true, Ordering::SeqCst);
        assert!(
            service
                .selftest(&server, 42, "BannerSelftest", "primeultra")
                .await
                .is_err()
        );
        let requests = mock.requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|(_, p, _)| p == "/proxy/run-command")
                .count(),
            1
        );
        assert!(
            requests
                .iter()
                .any(|(_, p, _)| p == "/channels/9000/messages")
        );
        assert!(requests.iter().any(|(_, p, v)| p == &review_path()
            && v["content"].as_str().is_some_and(|t| t.contains("FAILED"))));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn selftest_guild_failure_continues_prize_dm_and_reports_status() -> Result<()> {
        let (service, server, mock) = selftest_fixture("selftest_guild_fail").await?;
        *mock.guild.lock().await = json!({"banner":"a_animated"});
        assert!(
            service
                .selftest(&server, 42, "BannerSelftest", "primeultra")
                .await
                .is_ok()
        );
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
        );
        *mock.guild.lock().await = json!({"banner":"static"});
        mock.fail_image.store(true, Ordering::SeqCst);
        assert!(
            service
                .selftest(&server, 42, "BannerSelftest", "primeultra")
                .await
                .is_err()
        );
        let requests = mock.requests.lock().await;
        assert!(requests.iter().any(|(_, p, _)| p.ends_with("run-command")));
        assert_eq!(
            requests
                .iter()
                .filter(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
                .count(),
            2
        );
        assert!(requests.iter().any(|(_, p, v)| {
            p == &review_path()
                && v["content"].as_str().is_some_and(|t| {
                    t.contains("RESTORE FAILED")
                        && t.contains("https://discord.com/channels/917520262797344779/")
                })
        }));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn selftest_rejects_real_players_legend_and_active_contests() -> Result<()> {
        let (service, server, mock) = selftest_fixture("target_guard").await?;
        assert!(
            service
                .selftest(&server, 42, "Steve", "primeultra")
                .await
                .is_err()
        );
        assert!(
            service
                .selftest(&server, 42, "BannerSelftest", "legend")
                .await
                .is_err()
        );
        let schedule = super::super::model::Schedule::test(Utc::now().timestamp());
        let id = service.insert_contest(2030, 1, schedule, true).await?;
        service.set_state(id, "open").await?;
        assert!(
            service
                .selftest(&server, 42, "BannerSelftest", "primeultra")
                .await
                .is_err()
        );
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(m, p, _)| m == "PATCH" || p.ends_with("run-command"))
        );
        // Plain removetemp is never accepted by the shared proxy helper.
        assert!(
            server
                .banner_prize_command(
                    "00000000-0000-0000-0000-000000000002",
                    "primeultra",
                    "removetemp",
                    ""
                )
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn selftest_small_image_and_backup_failure_do_not_block_rank_dm() -> Result<()> {
        let (service, server, mock) = selftest_fixture("small_backup").await?;
        *mock.guild.lock().await = json!({"banner":"small"});
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(640, 360).write_to(&mut bytes, image::ImageFormat::Png)?;
        *mock.attachment.lock().await = bytes.into_inner();
        service
            .selftest(&server, 42, "BannerSelftest", "primeultra")
            .await?;
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
        );
        assert!(
            mock.requests
                .lock()
                .await
                .iter()
                .any(|(_, p, _)| p.ends_with("run-command"))
        );
        mock.requests.lock().await.clear();
        // A failed backup POST forbids every guild PATCH but the other parts still run.
        mock.failures
            .lock()
            .await
            .push(("POST".into(), review_path(), 4, 403));
        // Call image step directly so the fixture's failure targets the backup itself.
        let id = service
            .operational_journal("selftest-backup-failure")
            .await?;
        assert!(service.selftest_guild_images(id).await.is_err());
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn selftest_verification_failure_still_restores_originals() -> Result<()> {
        let (service, server, mock) = selftest_fixture("restore_verify").await?;
        *mock.guild.lock().await = json!({"banner":"changed_banner"});
        assert!(
            service
                .selftest(&server, 42, "BannerSelftest", "primeultra")
                .await
                .is_err()
        );
        let requests = mock.requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
                .count(),
            2
        );
        assert!(requests.iter().any(|(_, p, v)| {
            p == &review_path()
                && v["content"]
                    .as_str()
                    .is_some_and(|s| s.contains("restored and verified"))
        }));
        assert!(requests.iter().any(|(_, p, _)| p.ends_with("run-command")));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn gateway_stop_rearms_delivered_history_and_holds_uncertainty() -> Result<()> {
        let (service, _, mock) = fixture("rearm").await?;
        service.report("first stop").await?;
        service.flush_reports().await?;
        service.rearm_gateway_report().await?;
        service.report("second stop").await?;
        service.flush_reports().await?;
        assert_eq!(mock.created_posts.load(Ordering::SeqCst), 2);
        let id = service.operational_journal("gateway-stop").await?;
        let effects = service.effects(id).await?;
        assert_eq!(effects["report_gateway_stop"]["message"], "second stop");
        assert_eq!(
            effects
                .as_object()
                .unwrap()
                .keys()
                .filter(|k| k.starts_with("report_gateway_stop_history_"))
                .count(),
            1
        );
        service.rearm_gateway_report().await?;
        service.report("uncertain").await?;
        service.begin(id, "report_gateway_stop", "inspect").await?;
        let mut effects = service.effects(id).await?;
        effects["report_gateway_stop"]["dispatch_started"] = json!(true);
        sqlx::query("UPDATE banner_contests SET effects=? WHERE id=?")
            .bind(effects.to_string())
            .bind(id)
            .execute(&service.pool)
            .await?;
        service.report("restart same stop").await?;
        assert_eq!(
            service.effects(id).await?["report_gateway_stop"]["message"],
            "uncertain"
        );
        // Only confirmed health creates a new event, retaining the old uncertainty.
        service.rearm_gateway_report().await?;
        service.report("new stop after health").await?;
        service.flush_reports().await?;
        let effects = service.effects(id).await?;
        assert_eq!(
            effects["report_gateway_stop"]["message"],
            "new stop after health"
        );
        assert!(effects.as_object().unwrap().iter().any(|(k, v)| {
            k.starts_with("report_gateway_stop_history_")
                && v["message"] == "uncertain"
                && v["dispatch_started"] == true
        }));
        assert_eq!(mock.created_posts.load(Ordering::SeqCst), 3);
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn gateway_loop_stop_alert_deduplicates_and_holds_on_restart() -> Result<()> {
        let (service, server, mock) = fixture("gateway_report").await?;
        mock.failures
            .lock()
            .await
            .push(("POST".into(), review_path(), 1, 599));
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            service.gateway_loop(server.clone()),
        )
        .await?;
        // Interrupt the actual receipt write after Discord accepted the nonced message.
        sqlx::query("CREATE TRIGGER lose_gateway_receipt BEFORE UPDATE ON banner_contests FOR EACH ROW BEGIN IF NEW.contest_key='gateway-stop' AND JSON_UNQUOTE(JSON_EXTRACT(NEW.effects,'$.report_gateway_stop.state'))='done' THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='injected receipt loss'; END IF; END").execute(&service.pool).await?;
        assert!(service.poll(&server).await.is_err());
        assert_eq!(mock.created_posts.load(Ordering::SeqCst), 1);
        sqlx::query("DROP TRIGGER lose_gateway_receipt")
            .execute(&service.pool)
            .await?;
        let id: u64 =
            sqlx::query_scalar("SELECT id FROM banner_contests WHERE contest_key='gateway-stop'")
                .fetch_one(&service.pool)
                .await?;
        assert_eq!(
            service.effects(id).await?["report_gateway_stop"]["state"],
            "attempted"
        );
        service.clone().gateway_loop(server.clone()).await;
        service.poll(&server).await?;
        assert_eq!(mock.created_posts.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.effects(id).await?["report_gateway_stop"]["dispatch_started"],
            true
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn banner_schema_failure_does_not_disable_link_database() -> Result<()> {
        let (service, _, _) = fixture("schema_isolation").await?;
        sqlx::query("CREATE TABLE uuid_to_discord(uuid CHAR(36),discord_id VARCHAR(64))")
            .execute(&service.pool)
            .await?;
        sqlx::query(
            "INSERT INTO uuid_to_discord VALUES('00000000-0000-0000-0000-000000000001','42')",
        )
        .execute(&service.pool)
        .await?;
        sqlx::query("DROP TABLE banner_themes")
            .execute(&service.pool)
            .await?;
        sqlx::query("CREATE TABLE banner_themes(broken INT)")
            .execute(&service.pool)
            .await?;
        let env = crate::config::Environment::default();
        assert!(
            super::super::BannerService::initialize(
                service.pool.clone(),
                service.http.clone(),
                &env
            )
            .await
            .is_err()
        );
        let databases = crate::database::Databases {
            link: service.pool.clone(),
            stats: service.pool.clone(),
        };
        assert!(databases.mapping_for_discord("42").await?.is_some());
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn public_transient_retry_reuses_nonce_without_duplicate_post() -> Result<()> {
        let (service, _, mock) = fixture("public_retry").await?;
        let now = Utc::now().timestamp();
        let id = service
            .insert_contest(
                2030,
                1,
                Schedule {
                    call: now,
                    close: now + 86400,
                    voting: now + 129_600,
                    end: now + 388_800,
                },
                false,
            )
            .await?;
        let public = format!("/channels/{}/messages", super::super::ANNOUNCEMENTS);
        mock.failures
            .lock()
            .await
            .push(("POST".into(), public.clone(), 1, 599));
        service.call(&service.contest(id).await?).await?;
        assert_eq!(service.contest(id).await?.state, "open");
        assert_eq!(
            mock.created_posts.load(Ordering::SeqCst),
            1,
            "accepted post must be deduplicated on retry"
        );
        let requests = mock.requests.lock().await;
        let posts: Vec<_> = requests
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == &public)
            .collect();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0].2["nonce"], posts[1].2["nonce"]);
        assert_eq!(posts[0].2["enforce_nonce"], true);
        assert!(posts[0].2["nonce"].as_str().unwrap().len() <= 25);
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn patch_dm_fallback_fire_and_reaction_reads_retry_independently() -> Result<()> {
        let (service, server, mock) = fixture("private_stages").await?;
        let (id, entry) = opened(&service, false).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        service
            .decide(
                entry,
                "approve",
                &interaction(&format!("banner:approve:{entry}"), 3),
            )
            .await?;
        mock.failures
            .lock()
            .await
            .push(("PATCH".into(), review_path(), 1, 500));
        mock.failures
            .lock()
            .await
            .push(("POST".into(), "/users/@me/channels".into(), 1, 500));
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "open");
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(_, p, _)| p == "/channels/982192297645056040/messages"),
            "transient DM error must not immediately fall back"
        );
        mock.failures.lock().await.push((
            "POST".into(),
            "/channels/982192297645056040/messages".into(),
            1,
            500,
        ));
        retry_now(&service, id).await?;
        service.poll(&server).await?;
        retry_now(&service, id).await?;
        service.poll(&server).await?;
        assert_eq!(service.entry(entry).await?.review_revision, 1);
        mock.failures.lock().await.push((
            "PUT".into(),
            format!("/channels/{}/messages", super::super::ANNOUNCEMENTS),
            1,
            500,
        ));
        service.voting(&service.contest(id).await?).await?;
        assert_eq!(service.contest(id).await?.state, "voting");
        retry_now(&service, id).await?;
        service.poll(&server).await?;
        assert_eq!(
            serde_json::from_str::<Value>(
                &sqlx::query_scalar::<_, String>("SELECT effects FROM banner_contests WHERE id=?")
                    .bind(id)
                    .fetch_one(&service.pool)
                    .await?
            )?[format!("fire_{entry}")]["state"],
            "done"
        );
        service
            .done(
                id,
                "voting_public_at",
                json!(Utc::now().timestamp() - 86401),
            )
            .await?;
        sqlx::query("UPDATE banner_contests SET end_at=? WHERE id=?")
            .bind(Utc::now().timestamp() - 1)
            .bind(id)
            .execute(&service.pool)
            .await?;
        mock.failures.lock().await.push((
            "GET".into(),
            format!("/channels/{}/messages", super::super::ANNOUNCEMENTS),
            1,
            500,
        ));
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "voting");
        assert!(!mock.awarded.load(Ordering::SeqCst));
        retry_now(&service, id).await?;
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "complete");
        assert!(mock.awarded.load(Ordering::SeqCst));
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn floodgate_name_resolved_by_services_can_submit() -> Result<()> {
        let (service, server, _mock) = fixture("floodgate").await?;
        sqlx::query("ALTER TABLE player_info MODIFY name VARCHAR(17)")
            .execute(&service.pool)
            .await?;
        sqlx::query("UPDATE player_info SET name='.1234567890123456'")
            .execute(&service.pool)
            .await?;
        let now = Utc::now().timestamp();
        let id = service
            .insert_contest(
                2030,
                1,
                Schedule {
                    call: now,
                    close: now + 86400,
                    voting: now + 129_600,
                    end: now + 388_800,
                },
                false,
            )
            .await?;
        service.call(&service.contest(id).await?).await?;
        let mut form = interaction(&format!("banner:form:{id}"), 5);
        form["data"]["components"] = json!([
            {"type":18,"component":{"type":4,"custom_id":"username","value":".1234567890123456"}},
            {"type":18,"component":{"type":4,"custom_id":"email","value":"private@example.org"}},
            {"type":18,"component":{"type":19,"custom_id":"image","values":["777"]}}
        ]);
        form["data"]["resolved"] = json!({"attachments":{"777":{"url":"https://cdn.discordapp.com/attachments/1/2/test.png","size":10000}}});
        assert!(
            service
                .submit(id, &form, &server)
                .await?
                .starts_with("Thanks!")
        );
        assert_eq!(service.entries(id).await?[0].username, ".1234567890123456");
        assert_eq!(
            apply_modal(id)["data"]["components"][0]["component"]["max_length"],
            17
        );
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn general_fallback_keeps_denial_reason_per_owner() -> Result<()> {
        let (service, _, mock) = fixture("denial_reason").await?;
        let (id, entry) = opened(&service, false).await?;
        let mut other = interaction(&format!("banner:other:{entry}"), 5);
        other["data"]["components"] = json!([{"type":1,"components":[{"type":4,"custom_id":"reason","value":"Owner requested reason"}]}]);
        service.decide(entry, "other", &other).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        assert!(mock.requests.lock().await.iter().any(|(_, p, v)| {
            p == "/channels/982192297645056040/messages"
                && v["content"]
                    .as_str()
                    .is_some_and(|s| s.contains("denied: Owner requested reason."))
                && v["allowed_mentions"]["users"] == json!(["123"])
        }));
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn private_notifications_keep_retrying_after_public_phase_completes() -> Result<()> {
        let (service, server, mock) = fixture("terminal_notices").await?;
        let (id, entry) = opened(&service, false).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        mock.failures.lock().await.push((
            "POST".into(),
            "/channels/982192297645056040/messages".into(),
            1,
            500,
        ));
        service.voting(&service.contest(id).await?).await?;
        assert_eq!(service.contest(id).await?.state, "complete");
        retry_now(&service, id).await?;
        service.poll(&server).await?;
        let effects: Value = serde_json::from_str(
            &sqlx::query_scalar::<_, String>("SELECT effects FROM banner_contests WHERE id=?")
                .bind(id)
                .fetch_one(&service.pool)
                .await?,
        )?;
        assert_eq!(effects[format!("notify_{entry}_1")]["state"], "done");
        assert_eq!(service.contest(id).await?.state, "complete");
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn rapid_changed_decisions_preserve_each_notification() -> Result<()> {
        let (service, _, mock) = fixture("notice_revisions").await?;
        let (id, entry) = opened(&service, true).await?;
        let mut deny = interaction(&format!("banner:deny:{entry}"), 3);
        deny["data"]["values"] = json!(["Low quality"]);
        service.decide(entry, "deny", &deny).await?;
        service
            .decide(
                entry,
                "approve",
                &interaction(&format!("banner:approve:{entry}"), 3),
            )
            .await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        let requests = mock.requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|(_, _, p)| p["content"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("TEST decision notification:")))
                .count(),
            2
        );
        assert!(requests.iter().any(|(_, _, p)| {
            p["content"]
                .as_str()
                .is_some_and(|s| s.contains("denied: Low quality"))
        }));
        assert!(requests.iter().any(|(_, _, p)| {
            p["content"]
                .as_str()
                .is_some_and(|s| s.contains("was accepted"))
        }));
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn saved_application_returns_waiting_even_when_review_upload_is_down() -> Result<()> {
        let (service, server, mock) = fixture("saved_submit").await?;
        let now = Utc::now().timestamp();
        let id = service
            .insert_contest(
                2030,
                1,
                Schedule {
                    call: now,
                    close: now + 86400,
                    voting: now + 129_600,
                    end: now + 388_800,
                },
                false,
            )
            .await?;
        service.call(&service.contest(id).await?).await?;
        let mut form = interaction(&format!("banner:form:{id}"), 5);
        form["data"]["components"] = json!([
            {"type":18,"component":{"type":4,"custom_id":"username","value":"Steve"}},
            {"type":18,"component":{"type":4,"custom_id":"email","value":"private@example.org"}},
            {"type":18,"component":{"type":19,"custom_id":"image","values":["777"]}}
        ]);
        form["data"]["resolved"] = json!({"attachments":{"777":{"url":"https://cdn.discordapp.com/attachments/1/2/test.png","size":10000}}});
        mock.failures
            .lock()
            .await
            .push(("POST".into(), review_path(), 1, 500));
        let response = service.submit(id, &form, &server).await?;
        assert!(response.starts_with("Thanks! Your screenshot is waiting for review."));
        assert_eq!(service.entries(id).await?.len(), 1);
        assert!(
            !mock
                .requests
                .lock()
                .await
                .iter()
                .any(|(m, p, _)| m == "POST" && p == &review_path())
        );
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "open");
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn prize_is_dispatched_to_proxy_not_rank_service() -> Result<()> {
        let (_service, server, mock) = fixture("proxy_route").await?;
        server
            .grant_banner_prize("00000000-0000-0000-0000-000000000001", "primeultra")
            .await?;
        assert_eq!(
            mock.requests
                .lock()
                .await
                .iter()
                .filter(|(_, p, _)| p == "/proxy/run-command")
                .count(),
            1
        );
        assert_eq!(
            mock.requests
                .lock()
                .await
                .iter()
                .filter(|(_, p, _)| p == "/run-command")
                .count(),
            0
        );
        Ok(())
    }
    #[derive(Clone)]
    struct LogCapture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for LogCapture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn http_failure_logs_status_without_private_url_token_or_body() -> Result<()> {
        let (service, _, mock) = fixture("log_status").await?;
        mock.failures
            .lock()
            .await
            .push(("GET".into(), "/private-token-path".into(), 1, 500));
        let output = LogCapture(Arc::default());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::WARN)
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        assert!(
            service
                .request(reqwest::Method::GET, "/private-token-path", None)
                .await
                .is_err()
        );
        let log = String::from_utf8(output.0.lock().unwrap().clone())?;
        assert!(
            log.contains("http_status=500"),
            "HTTP status must be logged: {log}"
        );
        for private in ["private-token-path", "local-fake-token", mock.base.as_str()] {
            assert!(!log.contains(private));
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn review_upload_transient_error_does_not_stop_contest() -> Result<()> {
        let (service, server, mock) = fixture("review_retry").await?;
        let (id, entry) = opened(&service, false).await?;
        mock.failures
            .lock()
            .await
            .push(("POST".into(), review_path(), 1, 500));
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "open");
        assert!(service.entry(entry).await?.review_message_id.is_some());
        let posts = mock.requests.lock().await.len();
        service.poll(&server).await?;
        assert_eq!(
            mock.requests.lock().await.len(),
            posts,
            "successful live retry must not upload again"
        );
        retry_now(&service, id).await?;
        service.poll(&server).await?;
        assert!(service.entry(entry).await?.review_message_id.is_some());
        assert_eq!(service.contest(id).await?.state, "open");
        assert!(!mock.requests.lock().await.iter().any(|(_, _, p)| {
            p["content"]
                .as_str()
                .is_some_and(|s| s.contains("failed after"))
        }));
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn private_retry_cap_reports_once_and_keeps_other_entries() -> Result<()> {
        let (service, server, mock) = fixture("retry_cap").await?;
        let (id, entry) = opened(&service, false).await?;
        mock.failures
            .lock()
            .await
            .push(("POST".into(), review_path(), 5, 403));
        for _ in 0..6 {
            retry_now(&service, id).await?;
            service.poll(&server).await?;
        }
        assert!(service.entry(entry).await?.review_message_id.is_none());
        assert_eq!(service.contest(id).await?.state, "open");
        let before = mock.requests.lock().await.len();
        service.poll(&server).await?;
        assert_eq!(
            mock.requests.lock().await.len(),
            before,
            "exhausted upload and delivered report must not repeat"
        );
        let other = insert_entry(
            &service,
            id,
            "456",
            "Other",
            "00000000-0000-0000-0000-000000000002",
            1,
        )
        .await?;
        service.poll(&server).await?;
        assert!(service.entry(other).await?.review_message_id.is_some());
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn stale_catchup_has_no_public_posts_or_prizes() -> Result<()> {
        let (service, server, mock) = fixture("stale").await?;
        let now = Utc::now().timestamp();
        for (month, state, close, voting, end) in [
            (1, "scheduled", now - 1, now - 1, now - 1),
            (2, "review", now - 2, now - 1, now + 43199),
            (3, "voting", now - 3, now - 2, now - 1),
        ] {
            let id = service
                .insert_contest(
                    2031,
                    month,
                    Schedule {
                        call: now - 4,
                        close,
                        voting,
                        end,
                    },
                    false,
                )
                .await?;
            service.set_state(id, state).await?;
            service.poll(&server).await?;
            assert_eq!(service.contest(id).await?.state, "skipped");
        }
        assert!(
            mock.requests
                .lock()
                .await
                .iter()
                .all(|(_, p, _)| p.starts_with(&review_path()))
        );
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn voting_requires_twelve_hours_to_open_and_twenty_four_public_hours_to_award()
    -> Result<()> {
        let (service, server, mock) = fixture("voting_time").await?;
        let (id, entry) = opened(&service, false).await?;
        sqlx::query("UPDATE banner_submissions SET status='approved' WHERE id=?")
            .bind(entry)
            .execute(&service.pool)
            .await?;
        let now = Utc::now().timestamp();
        sqlx::query("UPDATE banner_contests SET state='review',voting_at=?,end_at=? WHERE id=?")
            .bind(now - 1)
            .bind(now + 43210)
            .bind(id)
            .execute(&service.pool)
            .await?;
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "voting");
        sqlx::query("UPDATE banner_contests SET end_at=? WHERE id=?")
            .bind(now - 1)
            .bind(id)
            .execute(&service.pool)
            .await?;
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "skipped");
        assert!(!mock.awarded.load(Ordering::SeqCst));
        assert!(!mock.requests.lock().await.iter().any(|(_, _, p)| {
            p["content"]
                .as_str()
                .is_some_and(|s| s.contains("The winner of"))
        }));
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn listing_entries_does_not_fetch_blobs() -> Result<()> {
        let (service, _, _mock) = fixture("metadata").await?;
        let (id, entry) = opened(&service, false).await?;
        // A column-level SELECT grant makes a blob read fail, rather than merely inspecting SQL text.
        sqlx::query("DROP USER IF EXISTS 'banner_metadata'@'127.0.0.1'")
            .execute(&service.pool)
            .await?;
        sqlx::query("CREATE USER 'banner_metadata'@'127.0.0.1' IDENTIFIED BY 'local-only'")
            .execute(&service.pool)
            .await?;
        sqlx::query("GRANT SELECT(id,contest_id,discord_id,username,uuid,email,prize,status,decider,reason,submitted_at,shuffle_key,review_message_id,vote_message_id,votes,revision,review_revision,review_closed) ON banner_test_fix_metadata.banner_submissions TO 'banner_metadata'@'127.0.0.1'").execute(&service.pool).await?;
        let url = std::env::var("BANNER_TEST_DATABASE_URL")?;
        let mut parsed = reqwest::Url::parse(&url)?;
        parsed.set_path("/banner_test_fix_metadata");
        parsed.set_username("banner_metadata").unwrap();
        parsed.set_password(Some("local-only")).unwrap();
        let mut limited = service.clone();
        limited.pool = sqlx::MySqlPool::connect(parsed.as_str()).await?;
        assert_eq!(limited.entries(id).await?.len(), 1);
        assert_eq!(limited.entry(entry).await?.id, entry);
        assert!(limited.entry_image(entry).await.is_err());
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn cleared_seed_theme_survives_schema_restart() -> Result<()> {
        let (service, _, _mock) = fixture("themes").await?;
        // Run the same SQL used by the staff command.
        super::super::commands::save_theme(&service.pool, 2026, 12, None).await?;
        super::super::ensure_schema(&service.pool).await?;
        assert_eq!(service.theme(2026, 12).await?, None);
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn same_decision_does_not_increment_revision_or_notify_twice() -> Result<()> {
        let (service, _, mock) = fixture("decision").await?;
        let (id, entry) = opened(&service, true).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        let approve = interaction(&format!("banner:approve:{entry}"), 3);
        service.decide(entry, "approve", &approve).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        let before = mock.requests.lock().await.len();
        let revision = service.entry(entry).await?.revision;
        service.decide(entry, "approve", &approve).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        assert_eq!(service.entry(entry).await?.revision, revision);
        assert_eq!(mock.requests.lock().await.len(), before);
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn ineligible_winner_has_no_public_promise() -> Result<()> {
        let (service, server, mock) = fixture("ineligible").await?;
        let (id, entry) = opened(&service, false).await?;
        sqlx::query("UPDATE banner_submissions SET status='approved' WHERE id=?")
            .bind(entry)
            .execute(&service.pool)
            .await?;
        service.voting(&service.contest(id).await?).await?;
        mock.ranks
            .lock()
            .await
            .insert("Steve".into(), vec!["legend".into()]);
        service.finish(&service.contest(id).await?, &server).await?;
        assert!(!mock.requests.lock().await.iter().any(|(_, _, p)| {
            p["content"]
                .as_str()
                .is_some_and(|s| s.contains("The winner of"))
        }));
        assert!(!mock.awarded.load(Ordering::SeqCst));
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn failure_report_survives_discord_outage_and_restart() -> Result<()> {
        let (service, server, mock) = fixture("reports").await?;
        let (id, _entry) = opened(&service, false).await?;
        mock.failures
            .lock()
            .await
            .push(("POST".into(), review_path(), 1, 500));
        let _ = service.fail(id, "Local failure diagnostic").await;
        service.poll(&server).await?;
        let restarted = service.clone();
        restarted.poll(&server).await?;
        assert!(
            mock.requests
                .lock()
                .await
                .iter()
                .filter(|(_, _, p)| p["content"] == "Local failure diagnostic")
                .count()
                >= 2
        );
        let before = mock.requests.lock().await.len();
        restarted.poll(&server).await?;
        assert_eq!(mock.requests.lock().await.len(), before);
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn monthly_generation_waits_for_a_staff_started_real_contest() -> Result<()> {
        let (service, _, _mock) = fixture("gate").await?;
        service.generate().await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM banner_contests")
                .fetch_one(&service.pool)
                .await?,
            0
        );
        opened(&service, true).await?;
        service.generate().await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM banner_contests WHERE dry_run=FALSE"
            )
            .fetch_one(&service.pool)
            .await?,
            0
        );
        opened(&service, false).await?;
        service.generate().await?;
        service.generate().await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM banner_contests WHERE dry_run=FALSE"
            )
            .fetch_one(&service.pool)
            .await?,
            2
        );
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn prize_uses_proxy_and_waits_for_async_rank_save() -> Result<()> {
        let (service, server, mock) = fixture("proxy").await?;
        let (_id, _entry) = opened(&service, false).await?;
        mock.verify_delay.store(2, Ordering::SeqCst);
        server
            .grant_banner_prize("00000000-0000-0000-0000-000000000001", "primeultra")
            .await?;
        assert!(server.verify_banner_prize("Steve", "primeultra").await);
        let requests = mock.requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|(_, p, _)| p == "/proxy/run-command")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|(_, p, _)| p == "/run-command")
                .count(),
            0
        );
        assert!(
            requests
                .iter()
                .filter(|(_, p, _)| p == "/get-ranks")
                .count()
                >= 3
        );
        drop(requests);
        mock.deny_command.store(true, Ordering::SeqCst);
        assert!(
            server
                .grant_banner_prize("00000000-0000-0000-0000-000000000001", "primeultra")
                .await
                .is_err()
        );
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn final_review_other_reset_waits_for_worker_and_decision_locks() -> Result<()> {
        let (service, server, _mock) = fixture("reset_lock").await?;
        let (id, entry) = opened(&service, true).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        for worker in [false, true] {
            let guard = service.lock.lock().await;
            let connection = service.acquire().await?.unwrap();
            let mut other = interaction(&format!("banner:deny:{entry}"), 3);
            other["id"] = json!(if worker { "778" } else { "777" });
            other["data"]["values"] = json!(["Other"]);
            let clone = service.clone();
            let server = server.clone();
            let task = tokio::spawn(async move { clone.receive(other, server).await });
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            assert!(!task.is_finished(), "Other reset bypassed serialization");
            if worker {
                service.done(id, "worker_receipt", json!(true)).await?;
            } else {
                service
                    .decide(
                        entry,
                        "approve",
                        &interaction(&format!("banner:approve:{entry}"), 3),
                    )
                    .await?;
            }
            super::super::BannerService::release(connection).await;
            drop(guard);
            task.await??;
            let effects = service.effects(id).await?;
            assert!(if worker {
                effects["worker_receipt"]["state"] == "done"
            } else {
                effects[format!("notice_{entry}_1")]["state"] == "pending"
            });
        }
        // A separate process has its own mutex but shares MariaDB's advisory lock.
        let connection = service.acquire().await?.unwrap();
        let mut other = interaction(&format!("banner:deny:{entry}"), 3);
        other["id"] = json!("779");
        other["data"]["values"] = json!(["Other"]);
        let mut independent = service.clone();
        independent.lock = std::sync::Arc::default();
        let task = tokio::spawn(async move { independent.receive(other, server).await });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!task.is_finished(), "Other reset bypassed advisory lock");
        service
            .done(id, "other_process_receipt", json!(true))
            .await?;
        super::super::BannerService::release(connection).await;
        task.await??;
        assert_eq!(
            service.effects(id).await?["other_process_receipt"]["state"],
            "done"
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn final_review_private_posts_deduplicate_live_and_hold_uncertain_restart() -> Result<()>
    {
        let (service, _, mock) = fixture("private_nonce").await?;
        let (id, entry) = opened(&service, false).await?;
        for kind in ["review", "notify", "report"] {
            let key = format!("{kind}_regression");
            service.begin(id, &key, "Inspect message").await?;
            let before = mock.created_posts.load(Ordering::SeqCst);
            mock.failures
                .lock()
                .await
                .push(("POST".into(), review_path(), 1, 599));
            service
                .journal_message(
                    id,
                    &key,
                    super::super::REVIEWS,
                    json!({"content":kind}),
                    None,
                )
                .await?;
            assert_eq!(mock.created_posts.load(Ordering::SeqCst), before + 1);
            retry_now(&service, id).await?;
            assert!(
                service
                    .clone()
                    .begin(id, &key, "Inspect message")
                    .await
                    .is_err(),
                "accepted but unrecorded message must not redispatch after restart"
            );
        }
        service
            .decide(entry, "deny", &{
                let mut i = interaction(&format!("banner:deny:{entry}"), 3);
                i["data"]["values"] = json!(["Low quality"]);
                i
            })
            .await?;
        mock.failures.lock().await.push((
            "POST".into(),
            format!("/channels/{}/messages", super::super::GENERAL),
            1,
            599,
        ));
        let before = mock.created_posts.load(Ordering::SeqCst);
        service
            .notify(&service.contest(id).await?, &service.entry(entry).await?)
            .await?;
        assert_eq!(mock.created_posts.load(Ordering::SeqCst), before + 1);
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn final_review_refusal_reports_are_durable_before_pause() -> Result<()> {
        for identity in [false, true] {
            let (service, server, mock) = fixture(if identity {
                "pause_identity"
            } else {
                "pause_rank"
            })
            .await?;
            let (id, entry) = opened(&service, false).await?;
            sqlx::query("UPDATE banner_submissions SET status='approved' WHERE id=?")
                .bind(entry)
                .execute(&service.pool)
                .await?;
            service.voting(&service.contest(id).await?).await?;
            // Simulate a stop/failure at the state write: a durable report must already exist.
            sqlx::query("CREATE TRIGGER interrupt_pause BEFORE UPDATE ON banner_contests FOR EACH ROW BEGIN IF NEW.state='paused' THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='simulated stop'; END IF; END").execute(&service.pool).await?;
            if identity {
                sqlx::query("DELETE FROM player_info")
                    .execute(&service.pool)
                    .await?;
            } else {
                mock.ranks
                    .lock()
                    .await
                    .insert("Steve".into(), vec!["legend".into()]);
            }
            assert!(
                service
                    .finish(&service.contest(id).await?, &server)
                    .await
                    .is_err()
            );
            let key = if identity {
                "report_identity"
            } else {
                "report_eligibility"
            };
            assert_eq!(service.effects(id).await?[key]["state"], "pending");
            sqlx::query("DROP TRIGGER interrupt_pause")
                .execute(&service.pool)
                .await?;
            service.clone().flush_reports().await?;
            assert_eq!(service.effects(id).await?[key]["state"], "done");
            let before = mock.created_posts.load(Ordering::SeqCst);
            service.flush_reports().await?;
            assert_eq!(mock.created_posts.load(Ordering::SeqCst), before);
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn other_cancel_resets_original_menu() -> Result<()> {
        let (service, server, mock) = fixture("other").await?;
        let (id, entry) = opened(&service, true).await?;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        let mut other = interaction(&format!("banner:deny:{entry}"), 3);
        other["data"]["values"] = json!(["Other"]);
        let before = mock.requests.lock().await.len();
        service.receive(other, server).await?;
        assert!(
            mock.requests.lock().await[before..]
                .iter()
                .any(|(m, p, v)| m == "PATCH"
                    && p.starts_with(&review_path())
                    && v["components"][1]["components"][0]["options"][3]["default"] != true)
        );
        Ok(())
    }
    #[tokio::test]
    #[ignore = "requires BANNER_TEST_DATABASE_URL pointing to an isolated local MariaDB banner_test database"]
    async fn schema_journal_forms_decisions_voting_catchup_and_prizes() -> Result<()> {
        crate::install_crypto_provider()?;
        let url = std::env::var("BANNER_TEST_DATABASE_URL").context("set local test DB URL")?;
        let parsed = reqwest::Url::parse(&url)?;
        assert!(
            matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
                && parsed.path().starts_with("/banner_test"),
            "only an isolated local DB is permitted"
        );
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await?;
        for table in [
            "banner_winners",
            "banner_submissions",
            "banner_contests",
            "banner_themes",
            "player_info",
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE IF EXISTS {table}")))
                .execute(&pool)
                .await?;
        }
        super::super::ensure_schema(&pool).await?;
        sqlx::query("UPDATE banner_themes SET theme='Custom' WHERE year=2026 AND month=10")
            .execute(&pool)
            .await?;
        super::super::ensure_schema(&pool).await?;
        let empty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM banner_contests")
            .fetch_one(&pool)
            .await?;
        assert_eq!(empty, 0);
        sqlx::query("CREATE TABLE player_info(uuid CHAR(36) PRIMARY KEY,name VARCHAR(16),first_join BIGINT)").execute(&pool).await?;
        sqlx::query(
            "INSERT INTO player_info VALUES('00000000-0000-0000-0000-000000000001','Steve',0),('00000000-0000-0000-0000-000000000004','Winner',0),('00000000-0000-0000-0000-000000000005','FailImage',0)",
        )
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO player_info VALUES('00000000-0000-0000-0000-000000000002','BannerSelftest',0)").execute(&pool).await?;
        let mock = Mock::start().await;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let env = Arc::new(crate::config::Environment {
            discord_token: "local-fake-token".into(),
            proxy_command_base_url: Some(format!("{}/proxy", mock.base)),
            proxy_command_access_token: Some("local-fake-token".into()),
            rank_service_base_url: Some(mock.base.clone()),
            rank_service_access_token: Some("local-fake-token".into()),
            ..Default::default()
        });
        let mut service = super::super::BannerService::new(pool.clone(), http.clone(), &env);
        service.discord_api = mock.base.clone();
        let server = crate::server::ServerService::new(
            http,
            env,
            Some(crate::database::Databases {
                link: pool.clone(),
                stats: pool.clone(),
            }),
        );
        assert_eq!(service.theme(2026, 10).await?.as_deref(), Some("Custom"));
        assert_eq!(service.theme(2026, 11).await?, None);
        service.generate().await?;
        service.generate().await?;
        let october: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM banner_contests WHERE contest_key='2026-10'")
                .fetch_one(&pool)
                .await?;
        assert_eq!(october, 0);
        assert!(
            mock.requests.lock().await.is_empty(),
            "generation must never post"
        );

        // Raw modal opening and the real docs-shaped form intake, with a local CDN fixture.
        let now = Utc::now().timestamp();
        let id = service
            .insert_contest(2026, 10, Schedule::test(now), true)
            .await?;
        service.call(&service.contest(id).await?).await?;
        service.call(&service.contest(id).await?).await?;
        service
            .receive(
                interaction(&format!("banner:apply:{id}"), 3),
                server.clone(),
            )
            .await?;
        let mut form = interaction(&format!("banner:form:{id}"), 5);
        form["data"]["components"] = json!([
            {"type":18,"component":{"type":4,"custom_id":"username","value":"Steve"}},
            {"type":18,"component":{"type":4,"custom_id":"email","value":"private@example.org"}},
            {"type":18,"component":{"type":19,"custom_id":"image","values":["777"]}}
        ]);
        form["data"]["resolved"] = json!({"attachments":{"777":{"url":"https://cdn.discordapp.com/ephemeral-attachments/1/2/test.png","size":10000}}});
        service.receive(form.clone(), server.clone()).await?;
        let entries = service.entries(id).await?;
        assert_eq!(entries.len(), 1);
        let first = entries[0].id;
        service
            .reconcile_entries(&service.contest(id).await?)
            .await?;
        service.receive(form.clone(), server.clone()).await?;
        assert_eq!(service.entries(id).await?.len(), 1);
        let mut denied = interaction(&format!("banner:deny:{first}"), 3);
        denied["data"]["values"] = json!(["Low quality"]);
        service.receive(denied, server.clone()).await?;
        assert_eq!(service.entry(first).await?.status, "denied");
        service.receive(form, server.clone()).await?;
        assert_eq!(
            service.entries(id).await?.len(),
            1,
            "denied cannot be replaced"
        );
        service
            .receive(
                interaction(&format!("banner:approve:{first}"), 3),
                server.clone(),
            )
            .await?;
        assert_eq!(service.entry(first).await?.status, "approved");
        let pending = insert_entry(
            &service,
            id,
            "456",
            "Unreviewed",
            "00000000-0000-0000-0000-000000000002",
            now + 1,
        )
        .await?;
        // Database constraints protect even concurrent/outside writers.
        let duplicate = insert_entry(
            &service,
            id,
            "789",
            "steve",
            "00000000-0000-0000-0000-000000000003",
            now + 2,
        )
        .await;
        assert!(duplicate.is_err());
        let duplicate = insert_entry(
            &service,
            id,
            "123",
            "OtherName",
            "00000000-0000-0000-0000-000000000003",
            now + 2,
        )
        .await;
        assert!(duplicate.is_err());
        let connection = service.acquire().await?.unwrap();
        assert!(service.acquire().await?.is_none());
        super::super::BannerService::release(connection).await;
        assert!(
            service
                .begin(id, "crash_fixture", "manual")
                .await?
                .is_none()
        );
        service
            .done(id, "crash_fixture", json!("persisted"))
            .await?;
        assert_eq!(
            service.begin(id, "crash_fixture", "manual").await?,
            Some(json!("persisted"))
        );
        let before = mock.requests.lock().await.len();
        sqlx::query("UPDATE banner_contests SET close_at=?,voting_at=?,end_at=? WHERE id=?")
            .bind(now - 3)
            .bind(now - 2)
            .bind(now - 1)
            .bind(id)
            .execute(&pool)
            .await?;
        // Restart uses the same pool/journal and catches up every missed phase.
        service.poll(&server).await?;
        assert_eq!(service.contest(id).await?.state, "complete");
        assert_eq!(service.entry(pending).await?.status, "expired");
        assert_eq!(service.entry(first).await?.votes, 100);
        assert!(!mock.awarded.load(Ordering::SeqCst));
        let requests = mock.requests.lock().await.clone();
        for (_, path, payload) in &requests {
            if path.contains("/channels/") {
                assert!(
                    path.starts_with(&format!("/channels/{}/", super::super::REVIEWS)),
                    "dry-run leaked to another channel"
                );
            }
            assert!(
                !path.starts_with("/guilds/") && path != "/run-command",
                "dry-run changed production state"
            );
            assert!(
                !payload.to_string().contains("private@example.org"),
                "email leaked"
            );
            if path.contains("/messages") && payload.is_object() {
                assert!(payload.get("allowed_mentions").is_some());
            }
        }
        assert!(requests.len() > before);
        let posts_before = requests
            .iter()
            .filter(|(m, p, _)| m == "POST" && p.contains("/messages"))
            .count();
        service.poll(&server).await?;
        let posts_after = mock
            .requests
            .lock()
            .await
            .iter()
            .filter(|(m, p, _)| m == "POST" && p.contains("/messages"))
            .count();
        assert_eq!(posts_before, posts_after, "completed flow must not repost");

        // A real-mode local stub verifies one Modify Guild call and one temporary rank.
        let real = service
            .insert_contest(2027, 1, Schedule::test(now), false)
            .await?;
        service.call(&service.contest(real).await?).await?;
        let winning = insert_entry(
            &service,
            real,
            "999",
            "Winner",
            "00000000-0000-0000-0000-000000000004",
            now,
        )
        .await?;
        sqlx::query("UPDATE banner_submissions SET status='approved' WHERE id=?")
            .bind(winning)
            .execute(&pool)
            .await?;
        sqlx::query("UPDATE banner_contests SET close_at=?,voting_at=?,end_at=? WHERE id=?")
            .bind(now - 3)
            .bind(now - 2)
            .bind(now - 1)
            .bind(real)
            .execute(&pool)
            .await?;
        service.voting(&service.contest(real).await?).await?;
        service
            .done(real, "voting_public_at", json!(now - 86401))
            .await?;
        service.poll(&server).await?;
        assert_eq!(service.contest(real).await?.state, "complete");
        assert!(mock.awarded.load(Ordering::SeqCst));
        let requests = mock.requests.lock().await.clone();
        let patches: Vec<_> = requests
            .iter()
            .filter(|(m, p, _)| m == "PATCH" && p.starts_with("/guilds/"))
            .collect();
        assert_eq!(patches.len(), 1);
        assert_eq!(
            *mock.audit_reasons.lock().await,
            vec!["banner contest winner"]
        );
        assert_eq!(patches[0].2["banner"], patches[0].2["splash"]);
        assert_eq!(patches[0].2["banner"], patches[0].2["discovery_splash"]);
        let commands: Vec<_> = requests
            .iter()
            .filter(|(_, p, _)| p == "/proxy/run-command")
            .collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0].2["command"],
            "lpv user 00000000-0000-0000-0000-000000000004 parent addtemp primeultra 1mo"
        );
        // Force closed DMs; fallback may mention only the affected account.
        let c = service.contest(real).await?;
        let e = service.entry(winning).await?;
        service.notify(&c, &e).await?;
        assert!(
            mock.requests
                .lock()
                .await
                .iter()
                .any(|(_, p, v)| p == "/channels/982192297645056040/messages"
                    && v["allowed_mentions"]["users"] == json!(["999"]))
        );
        // Explicit failure does not cause an endless retry of guild image changes.
        mock.awarded.store(false, Ordering::SeqCst);
        mock.fail_image.store(true, Ordering::SeqCst);
        let broken = service
            .insert_contest(2027, 2, Schedule::test(now), false)
            .await?;
        service.call(&service.contest(broken).await?).await?;
        let e = insert_entry(
            &service,
            broken,
            "888",
            "FailImage",
            "00000000-0000-0000-0000-000000000005",
            now,
        )
        .await?;
        sqlx::query("UPDATE banner_submissions SET status='approved' WHERE id=?")
            .bind(e)
            .execute(&pool)
            .await?;
        sqlx::query("UPDATE banner_contests SET close_at=?,voting_at=?,end_at=? WHERE id=?")
            .bind(now - 3)
            .bind(now - 2)
            .bind(now - 1)
            .bind(broken)
            .execute(&pool)
            .await?;
        service.voting(&service.contest(broken).await?).await?;
        service
            .done(broken, "voting_public_at", json!(now - 86401))
            .await?;
        service.poll(&server).await?;
        service.poll(&server).await?;
        assert_eq!(service.contest(broken).await?.state, "complete");
        assert!(mock.requests.lock().await.iter().any(|(_, p, v)| {
            p == &format!("/channels/{}/messages", super::super::REVIEWS)
                && v["content"]
                    .as_str()
                    .is_some_and(|s| s.contains("all three guild image slots"))
        }));
        // Rows inserted by an operator follow the same catch-up state machine.
        let direct = sqlx::query("INSERT INTO banner_contests(contest_key,year,month,call_at,close_at,voting_at,end_at,dry_run) VALUES('sql-fixture',2027,3,?,?,?,?,TRUE)").bind(now-4).bind(now-3).bind(now-2).bind(now-1).execute(&pool).await?.last_insert_id();
        service.poll(&server).await?;
        assert_eq!(service.contest(direct).await?.state, "complete");
        assert!(
            mock.requests
                .lock()
                .await
                .iter()
                .any(|(_, _, v)| v["content"] == super::super::NONE)
        );
        // Crash between an attempted post and its receipt must not resend it.
        let uncertain = service
            .insert_contest(2027, 4, Schedule::test(now), true)
            .await?;
        assert!(
            service
                .begin(uncertain, "call", "Inspect and reconcile the call message.")
                .await?
                .is_none()
        );
        service.poll(&server).await?;
        assert_eq!(service.contest(uncertain).await?.state, "paused");
        let before = mock.requests.lock().await.len();
        service.poll(&server).await?;
        assert_eq!(
            mock.requests.lock().await.len(),
            before,
            "failed effect must not retry forever"
        );
        pool.close().await;
        Ok(())
    }
}

#[test]
fn floodgate_names_pass_syntax_validation() {
    assert!(valid_username(".Bedrock_Player"));
    assert!(valid_username(".1234567890123456"));
}
#[test]
fn excessive_pixel_count_is_rejected_before_decode() {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(16384, 4095)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    assert!(crop(bytes.get_ref()).is_err());
}
#[test]
fn guild_crop_has_small_upload_budget() {
    assert!(std::hint::black_box(super::image::MAX_GUILD_IMAGE) <= 3 * 1024 * 1024);
}
#[tokio::test]
async fn discord_token_matches_serenity_normalization() {
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .connect_lazy("mysql://root@127.0.0.1/banner_test")
        .unwrap();
    let env = crate::config::Environment {
        discord_token: " Bot fake-token\n".into(),
        ..Default::default()
    };
    let service = super::BannerService::new(pool, reqwest::Client::new(), &env);
    assert_eq!(service.token.as_str(), "fake-token");
}

#[test]
fn serenity_modal_rationale_matches_actual_decoder() {
    let typed: poise::serenity_prelude::ModalInteractionData = serde_json::from_value(json!({
        "custom_id":"banner:form:1", "components":[{"type":18,"component":{"type":4,"custom_id":"username","value":"Steve"}}],
        "resolved":{"attachments":{"1":{"url":"private"}}}
    })).unwrap();
    assert!(typed.components[0].components.is_empty());
    let docs = include_str!("../../docs/banner-contest.md");
    assert!(docs.contains("silently drops username/email/file values"));
    assert!(!include_str!("../main.rs").contains("serenity::gateway::ws=off"));
}
#[test]
fn banner_component_guard_precedes_generic_handlers() {
    // Architectural regression: banner ownership must be explicit before any generic handler.
    let source = include_str!("../events/interactions.rs");
    let guard = source
        .find("c.data.custom_id.starts_with(\"banner:\")")
        .expect("explicit banner guard");
    assert!(
        guard
            < source
                .find("if let Some(service) = &data.event_submissions")
                .unwrap()
    );
    assert!(guard < source.find("approval(ctx, data, component)").unwrap());
}
