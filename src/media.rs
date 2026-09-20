use std::collections::HashMap;

use anyhow::{Context as _, Result};
use tokio::sync::{Mutex, RwLock};

use crate::database::Databases;

const DEFAULT_FREQUENCY: u16 = 3;
pub const MIN_FREQUENCY: u16 = 1;
pub const MAX_FREQUENCY: u16 = 100;

const FREQUENCY_STATE_KEY: &str = "media_channel_frequency";

pub struct MediaState {
    frequency: RwLock<u16>,
    counts: Mutex<HashMap<poise::serenity_prelude::ChannelId, u16>>,
    databases: Option<Databases>,
}

impl MediaState {
    pub async fn load(databases: Option<Databases>) -> Result<Self> {
        let frequency = if let Some(databases) = &databases {
            databases
                .state_value(FREQUENCY_STATE_KEY)
                .await
                .context("failed to load media channel frequency")?
                .and_then(|value| value.parse().ok())
                .map_or(DEFAULT_FREQUENCY, normalize)
        } else {
            tracing::warn!(
                "MySQL is unavailable; media channel frequency changes will reset on restart"
            );
            DEFAULT_FREQUENCY
        };
        Ok(Self {
            frequency: RwLock::new(frequency),
            counts: Mutex::new(HashMap::new()),
            databases,
        })
    }

    pub async fn frequency(&self) -> u16 {
        *self.frequency.read().await
    }

    pub async fn set_frequency(&self, frequency: u16) -> Result<()> {
        let frequency = normalize(frequency);
        if let Some(databases) = &self.databases {
            databases
                .set_state_value(FREQUENCY_STATE_KEY, &frequency.to_string())
                .await
                .context("failed to save media channel frequency")?;
        }
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
