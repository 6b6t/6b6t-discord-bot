//! Serenity 0.12.5 silently drops modal username/email/file values and resolved attachments.
//! It cannot expose original `INTERACTION_CREATE` JSON for Label/File Upload.
//! A minimal, uncompressed gateway session handles ONLY banner interactions. The
//! normal Serenity session still owns all other events and commands. No intent is
//! needed for guild interactions. Resume preserves missed events after reconnect.
use super::BannerService;
use crate::server::ServerService;
use anyhow::{Context as _, Result, bail};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Default)]
struct Session {
    id: Option<String>,
    url: Option<String>,
    sequence: Option<u64>,
    healthy: bool,
    stopped: Option<String>,
}
impl BannerService {
    pub(super) async fn gateway_loop(&self, server: ServerService) {
        // The Serenity session identified just before Ready. Respect the shared
        // five-second identify bucket before opening the raw session.
        tokio::time::sleep(Duration::from_secs(6)).await;
        let mut session = Session::default();
        let mut delay = 1;
        loop {
            let result = self.gateway_session(&server, &mut session).await;
            if let Some(reason) = &session.stopped {
                // Stop identifying; retain the report until Discord accepts it.
                loop {
                    if self.report(reason).await.is_ok() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
            delay = reconnect_delay(delay, session.healthy, result.is_ok());
            session.healthy = false;
            if result.is_err() {
                // No gateway payloads, interaction tokens or user inputs in logs.
                tracing::warn!("banner raw gateway disconnected; reconnecting with resume");
            }
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }
    async fn reserve_identify(&self) -> Result<bool> {
        let _guard = self.lock.lock().await;
        let Some(connection) = self.acquire().await? else {
            bail!("identify reservation busy");
        };
        let result = async {
            let now = chrono::Utc::now().timestamp();
            sqlx::query("DELETE FROM banner_gateway_identifies WHERE attempted_at<=?")
                .bind(now - 86400)
                .execute(&self.pool)
                .await?;
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM banner_gateway_identifies")
                .fetch_one(&self.pool)
                .await?;
            if count >= 20 {
                return Ok(false);
            }
            sqlx::query("INSERT INTO banner_gateway_identifies(attempted_at) VALUES(?)")
                .bind(now)
                .execute(&self.pool)
                .await?;
            Ok(true)
        }
        .await;
        Self::release(connection).await;
        result
    }
    async fn gateway_endpoint(&self, session: &mut Session) -> Result<String> {
        let fresh = session.id.is_none() || session.sequence.is_none();
        if fresh {
            let info = self
                .request(reqwest::Method::GET, "/gateway/bot", None)
                .await?;
            let remaining = info["session_start_limit"]["remaining"]
                .as_u64()
                .context("missing identify budget")?;
            if remaining < 100 {
                session.stopped = Some("Banner raw gateway stopped: Discord IDENTIFY remaining budget below 100. Check gateway session limits before restarting.".into());
                bail!("identify budget exhausted");
            }
            Ok(info["url"]
                .as_str()
                .context("missing gateway URL")?
                .to_owned())
        } else {
            session.url.clone().context("missing resume URL")
        }
    }
    async fn gateway_session(&self, server: &ServerService, session: &mut Session) -> Result<()> {
        let gateway = self.gateway_endpoint(session).await?;
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(30),
            connect_async(format!(
                "{}/?v=10&encoding=json",
                gateway.trim_end_matches('/')
            )),
        )
        .await
        .context("gateway connect timed out")??;
        let (mut sender, mut receiver) = socket.split();
        let hello = tokio::time::timeout(Duration::from_secs(30), receiver.next())
            .await
            .context("gateway hello timed out")?
            .context("gateway closed before hello")??;
        let hello: Value = serde_json::from_str(hello.to_text()?)?;
        if hello["op"] != 10 {
            bail!("expected gateway hello");
        }
        let heartbeat_ms = hello["d"]["heartbeat_interval"]
            .as_u64()
            .context("heartbeat interval missing")?;
        if heartbeat_ms == 0 {
            bail!("invalid heartbeat interval");
        }
        let identify = if let (Some(id), Some(sequence)) = (&session.id, session.sequence) {
            json!({"op":6,"d":{"token":*self.token,"session_id":id,"seq":sequence}})
        } else {
            if !self.reserve_identify().await? {
                session.stopped = Some("Banner raw gateway stopped: 20 fresh IDENTIFY attempts in 24 hours. Check session failures before restarting.".into());
                bail!("local identify budget exhausted");
            }
            json!({"op":2,"d":{"token":*self.token,"intents":0,"properties":{"os":std::env::consts::OS,"browser":"6b6t-banner-raw","device":"6b6t-banner-raw"}}})
        };
        sender
            .send(Message::Text(identify.to_string().into()))
            .await?;
        let mut heartbeat = tokio::time::interval(Duration::from_millis(heartbeat_ms));
        let mut acknowledged = true;
        loop {
            tokio::select! {
                _=heartbeat.tick()=>{
                    if !acknowledged {bail!("heartbeat ACK missing");}
                    sender.send(Message::Text(json!({"op":1,"d":session.sequence}).to_string().into())).await?;
                    acknowledged=false;
                }
                message=receiver.next()=>{
                    let message=message.context("gateway closed")??;
                    match message {
                        Message::Ping(data)=>{sender.send(Message::Pong(data)).await?;continue;}
                        Message::Pong(_)=>continue,
                        Message::Close(frame)=>{
                            if let Some(frame) = frame {
                                let code = u16::from(frame.code);
                                tracing::warn!(close_code = code, "banner gateway closed");
                                if fatal_close(code) { session.stopped = Some(format!("Banner raw gateway stopped on fatal close code {code}. Check credentials/gateway configuration.")); }
                                if matches!(code,4007|4009) { session.id = None; session.sequence = None; }
                            }
                            bail!("gateway close");
                        }
                        _=>{}
                    }
                    let event:Value=serde_json::from_str(message.to_text()?)?;
                    if let Some(sequence)=event["s"].as_u64() {session.sequence=Some(sequence);}
                    match event["op"].as_u64() {
                        Some(11)=>acknowledged=true,
                        Some(1)=>{sender.send(Message::Text(json!({"op":1,"d":session.sequence}).to_string().into())).await?;}
                        Some(7)=>return Ok(()),
                        Some(9)=>{
                            if event["d"]!=true {session.id=None;session.sequence=None;}
                            bail!("invalid gateway session");
                        }
                        Some(0)=>{
                            if event["t"]=="READY" || event["t"]=="RESUMED" { session.healthy = true; }
                            if event["t"]=="READY" {
                                tracing::info!("banner raw gateway ready");
                                session.id=event["d"]["session_id"].as_str().map(str::to_owned);
                                session.url=event["d"]["resume_gateway_url"].as_str().map(str::to_owned);
                            }
                            if event["t"]=="INTERACTION_CREATE" && event["d"]["data"]["custom_id"].as_str().is_some_and(|id|id.starts_with("banner:")) {
                                let service=self.clone();let server=server.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = service.receive(event["d"].clone(),server).await {
                                        let status = error.downcast_ref::<super::DiscordHttpError>().map(|e| e.0);
                                        tracing::error!(http_status = status, "banner raw interaction failed");
                                    }
                                });
                            }
                        }
                        _=>{}
                    }
                }
            }
        }
    }
}

fn fatal_close(code: u16) -> bool {
    matches!(code, 4004 | 4010..=4014)
}
fn reconnect_delay(previous: u64, healthy: bool, clean: bool) -> u64 {
    if healthy || clean {
        1
    } else {
        (previous * 2).clamp(6, 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn identify_limits_survive_restart_and_check_discord_remaining() -> Result<()> {
        let (service, server, _mock) =
            super::super::tests::local_integration::fixture("identify").await?;
        let mut session = Session::default();
        assert!(
            service
                .gateway_session(&server, &mut session)
                .await
                .is_err()
        );
        assert!(
            session.stopped.is_some(),
            "low Discord budget must stop before connecting"
        );
        for _ in 0..20 {
            assert!(service.reserve_identify().await?);
        }
        assert!(
            !service.clone().reserve_identify().await?,
            "restart must retain rolling cap"
        );
        sqlx::query("UPDATE banner_gateway_identifies SET attempted_at=?")
            .bind(chrono::Utc::now().timestamp() - 86401)
            .execute(&service.pool)
            .await?;
        assert!(service.reserve_identify().await?);
        Ok(())
    }
    #[tokio::test]
    async fn fatal_close_stops_raw_gateway_and_exposes_only_code() -> Result<()> {
        crate::install_crypto_provider()?;
        for code in [4004, 4010, 4011, 4012, 4013, 4014] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let url = format!("ws://{}", listener.local_addr()?);
            let task = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket
                    .send(Message::Text(
                        json!({"op":10,"d":{"heartbeat_interval":1000}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                let _ = socket.next().await;
                socket
                    .send(Message::Close(Some(
                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: code.into(),
                            reason: "private-close-body".into(),
                        },
                    )))
                    .await
                    .unwrap();
            });
            let env = Arc::new(crate::config::Environment::default());
            let pool = sqlx::mysql::MySqlPoolOptions::new()
                .connect_lazy("mysql://root@127.0.0.1:33309/banner_test")?;
            let http = reqwest::Client::new();
            let service = BannerService::new(pool, http.clone(), &env);
            let server = ServerService::new(http, env, None);
            let mut session = Session {
                id: Some("local".into()),
                sequence: Some(1),
                url: Some(url),
                ..Default::default()
            };
            assert!(
                service
                    .gateway_session(&server, &mut session)
                    .await
                    .is_err()
            );
            let report = session.stopped.context("fatal close must stop")?;
            assert!(report.contains(&code.to_string()));
            assert!(!report.contains("private-close-body"));
            task.await?;
        }
        Ok(())
    }
    #[tokio::test]
    #[ignore = "local MariaDB"]
    async fn healthy_ready_resets_real_reconnect_backoff() -> Result<()> {
        use std::sync::atomic::Ordering;
        let (service, server, mock) =
            super::super::tests::local_integration::fixture("backoff").await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("ws://{}", listener.local_addr()?);
        mock.gateway_remaining.store(1000, Ordering::SeqCst);
        *mock.gateway_url.lock().await = url.clone();
        let runner = tokio::spawn(async move {
            service.gateway_loop(server).await;
        });
        let (socket, _) =
            tokio::time::timeout(Duration::from_secs(10), listener.accept()).await??;
        let mut socket = tokio_tungstenite::accept_async(socket).await?;
        socket
            .send(Message::Text(
                json!({"op":10,"d":{"heartbeat_interval":1000}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _ = socket.next().await;
        socket.send(Message::Text(json!({"op":0,"s":1,"t":"READY","d":{"session_id":"ready","resume_gateway_url":url}}).to_string().into())).await?;
        socket
            .send(Message::Close(Some(
                tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: 4000.into(),
                    reason: "".into(),
                },
            )))
            .await?;
        let next = tokio::time::timeout(Duration::from_secs(3), listener.accept()).await;
        runner.abort();
        assert!(
            next.is_ok(),
            "healthy READY must reset reconnect to one second"
        );
        Ok(())
    }

    #[tokio::test]
    async fn raw_gateway_heartbeats_and_resumes_without_typed_modal_decoding() -> Result<()> {
        crate::install_crypto_provider()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("ws://{}", listener.local_addr()?);
        let resume_url = url.clone();
        let gateway = tokio::spawn(async move {
            for phase in 0..2 {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket
                    .send(Message::Text(
                        json!({"op":10,"d":{"heartbeat_interval":100}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                let auth: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                if phase == 0 {
                    assert_eq!(auth["op"], 6);

                    socket.send(Message::Text(json!({"op":0,"s":42,"t":"READY","d":{"session_id":"local-session","resume_gateway_url":resume_url}}).to_string().into())).await.unwrap();
                    let heartbeat: Value = serde_json::from_str(
                        socket.next().await.unwrap().unwrap().to_text().unwrap(),
                    )
                    .unwrap();
                    assert_eq!(heartbeat["op"], 1);
                    socket
                        .send(Message::Text(json!({"op":11}).to_string().into()))
                        .await
                        .unwrap();
                    socket
                        .send(Message::Text(json!({"op":7}).to_string().into()))
                        .await
                        .unwrap();
                } else {
                    assert_eq!(auth["op"], 6);
                    assert_eq!(auth["d"]["session_id"], "local-session");
                    assert_eq!(auth["d"]["seq"], 42);
                    socket
                        .send(Message::Text(
                            json!({"op":0,"s":43,"t":"RESUMED","d":{}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    socket
                        .send(Message::Text(json!({"op":9,"d":false}).to_string().into()))
                        .await
                        .unwrap();
                }
            }
        });
        let env = Arc::new(crate::config::Environment::default());
        let http = reqwest::Client::new();
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .connect_lazy("mysql://root@127.0.0.1:33307/banner_test")?;
        let service = BannerService::new(pool, http.clone(), &env);
        let server = ServerService::new(http, env, None);
        let mut session = Session {
            id: Some("initial-session".into()),
            url: Some(url),
            sequence: Some(0),
            ..Default::default()
        };
        service.gateway_session(&server, &mut session).await?;
        assert_eq!(session.id.as_deref(), Some("local-session"));
        assert_eq!(session.sequence, Some(42));
        session.healthy = false;
        assert!(
            service
                .gateway_session(&server, &mut session)
                .await
                .is_err()
        );
        assert!(
            session.healthy,
            "RESUMED must reset backoff even before an invalid session"
        );
        assert!(session.id.is_none() && session.sequence.is_none());
        gateway.await?;
        Ok(())
    }
}
