//! Serenity 0.12 cannot expose original `INTERACTION_CREATE` JSON for Label/File Upload.
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
}
impl BannerService {
    pub(super) async fn gateway_loop(&self, server: ServerService) {
        // The Serenity session identified just before Ready. Respect the shared
        // five-second identify bucket before opening the raw session.
        tokio::time::sleep(Duration::from_secs(6)).await;
        let mut session = Session::default();
        let mut delay = 1;
        loop {
            if let Ok(()) = self.gateway_session(&server, &mut session).await {
                delay = 1;
            } else {
                // No gateway payloads, interaction tokens or user inputs in logs.
                tracing::warn!("banner raw gateway disconnected; reconnecting with resume");
                delay = (delay * 2).clamp(6, 60);
            }
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }
    async fn gateway_session(&self, server: &ServerService, session: &mut Session) -> Result<()> {
        let gateway = if let Some(url) = &session.url {
            url.clone()
        } else {
            self.request(reqwest::Method::GET, "/gateway/bot", None)
                .await?["url"]
                .as_str()
                .context("missing gateway URL")?
                .to_owned()
        };
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
                            if frame.is_some_and(|frame|matches!(u16::from(frame.code),4007|4009)) {*session=Session::default();}
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
                            if event["d"]!=true {*session=Session::default();}
                            bail!("invalid gateway session");
                        }
                        Some(0)=>{
                            if event["t"]=="READY" {
                                tracing::info!("banner raw gateway ready");
                                session.id=event["d"]["session_id"].as_str().map(str::to_owned);
                                session.url=event["d"]["resume_gateway_url"].as_str().map(str::to_owned);
                            }
                            if event["t"]=="INTERACTION_CREATE" && event["d"]["data"]["custom_id"].as_str().is_some_and(|id|id.starts_with("banner:")) {
                                let service=self.clone();let server=server.clone();
                                tokio::spawn(async move {
                                    if service.receive(event["d"].clone(),server).await.is_err() {tracing::error!("banner raw interaction failed");}
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
                    assert_eq!(auth["op"], 2);
                    assert_eq!(auth["d"]["intents"], 0);
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
            id: None,
            url: Some(url),
            sequence: None,
        };
        service.gateway_session(&server, &mut session).await?;
        assert_eq!(session.id.as_deref(), Some("local-session"));
        assert_eq!(session.sequence, Some(42));
        assert!(
            service
                .gateway_session(&server, &mut session)
                .await
                .is_err()
        );
        assert!(session.id.is_none() && session.sequence.is_none());
        gateway.await?;
        Ok(())
    }
}
