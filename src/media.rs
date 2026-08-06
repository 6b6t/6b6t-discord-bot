use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

const DEFAULT_FREQUENCY: u16 = 3;
pub const MIN_FREQUENCY: u16 = 1;
pub const MAX_FREQUENCY: u16 = 100;

#[derive(Debug, Deserialize, Serialize)]
struct MediaSettings {
    frequency: u16,
}

pub struct MediaState {
    frequency: RwLock<u16>,
    counts: Mutex<HashMap<poise::serenity_prelude::ChannelId, u16>>,
    path: PathBuf,
}

impl MediaState {
    pub async fn load() -> Result<Self> {
        let path = PathBuf::from("data/media-channel-settings.json");
        let frequency = match tokio::fs::read_to_string(&path).await {
            Ok(content) => serde_json::from_str::<MediaSettings>(&content)
                .map_or(DEFAULT_FREQUENCY, |settings| normalize(settings.frequency)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => DEFAULT_FREQUENCY,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "failed to read media channel settings; using the default frequency");
                DEFAULT_FREQUENCY
            }
        };
        Ok(Self {
            frequency: RwLock::new(frequency),
            counts: Mutex::new(HashMap::new()),
            path,
        })
    }

    pub async fn frequency(&self) -> u16 {
        *self.frequency.read().await
    }

    pub async fn set_frequency(&self, frequency: u16) -> Result<()> {
        let frequency = normalize(frequency);
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("failed to create data directory")?;
        }
        let content = serde_json::to_string_pretty(&MediaSettings { frequency })?;
        tokio::fs::write(&self.path, content)
            .await
            .context("failed to save media channel settings")?;
        *self.frequency.write().await = frequency;
        Ok(())
    }

    pub async fn should_remind(&self, channel_id: poise::serenity_prelude::ChannelId) -> bool {
        let frequency = self.frequency().await;
        let mut counts = self.counts.lock().await;
        let count = counts.entry(channel_id).or_default();
        *count += 1;
        if *count >= frequency {
            *count = 0;
            true
        } else {
            false
        }
    }
}

fn normalize(frequency: u16) -> u16 {
    frequency.clamp(MIN_FREQUENCY, MAX_FREQUENCY)
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_FREQUENCY, MAX_FREQUENCY, MIN_FREQUENCY, normalize};

    #[test]
    fn frequency_is_bounded() {
        assert_eq!(normalize(0), MIN_FREQUENCY);
        assert_eq!(normalize(DEFAULT_FREQUENCY), DEFAULT_FREQUENCY);
        assert_eq!(normalize(u16::MAX), MAX_FREQUENCY);
    }
}
