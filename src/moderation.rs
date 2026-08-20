use std::{collections::HashMap, time::Duration};

use poise::serenity_prelude as serenity;
use tokio::sync::Mutex;
use uuid::Uuid;

const APPROVAL_TTL: Duration = Duration::from_hours(1);

/// The guild image a submission targets: the server banner, the invite splash,
/// or the Server Discovery splash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, poise::ChoiceParameter)]
pub enum BannerLocation {
    #[name = "Server banner"]
    Banner,
    #[name = "Invite splash"]
    Splash,
    #[name = "Discovery splash"]
    DiscoverySplash,
}

impl BannerLocation {
    /// Lowercase label for user-facing messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Banner => "server banner",
            Self::Splash => "server invite splash",
            Self::DiscoverySplash => "server Discovery splash",
        }
    }

    /// Audit log title for a completed change.
    pub fn change_title(self) -> &'static str {
        match self {
            Self::Banner => "Server Banner Changed",
            Self::Splash => "Server Invite Splash Changed",
            Self::DiscoverySplash => "Server Discovery Splash Changed",
        }
    }
}

#[derive(Clone, Debug)]
pub enum ApprovalAction {
    GuildImage {
        image_url: String,
        location: BannerLocation,
    },
    Ban {
        target_id: serenity::UserId,
        reason: String,
        delete_message_days: u8,
    },
    Unban {
        target_id: serenity::UserId,
        reason: String,
    },
    MediaFrequency {
        current: u16,
        requested: u16,
    },
    MiniTerminator {
        target_id: serenity::UserId,
        add: bool,
    },
}

#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    pub id: Uuid,
    pub submitter_id: serenity::UserId,
    pub submitter_tag: String,
    pub guild_id: serenity::GuildId,
    pub action: ApprovalAction,
    created_at: std::time::Instant,
}

#[derive(Default)]
pub struct PendingApprovals {
    requests: Mutex<HashMap<Uuid, ApprovalRequest>>,
}

impl PendingApprovals {
    pub async fn create(
        &self,
        submitter_id: serenity::UserId,
        submitter_tag: String,
        guild_id: serenity::GuildId,
        action: ApprovalAction,
    ) -> ApprovalRequest {
        let request = ApprovalRequest {
            id: Uuid::new_v4(),
            submitter_id,
            submitter_tag,
            guild_id,
            action,
            created_at: std::time::Instant::now(),
        };
        let mut requests = self.requests.lock().await;
        requests.retain(|_, request| request.created_at.elapsed() <= APPROVAL_TTL);
        requests.insert(request.id, request.clone());
        request
    }

    pub async fn get(&self, id: Uuid) -> Option<ApprovalRequest> {
        let mut requests = self.requests.lock().await;
        requests.retain(|_, request| request.created_at.elapsed() <= APPROVAL_TTL);
        requests.get(&id).cloned()
    }

    pub async fn remove(&self, id: Uuid) -> Option<ApprovalRequest> {
        let mut requests = self.requests.lock().await;
        requests.retain(|_, request| request.created_at.elapsed() <= APPROVAL_TTL);
        requests.remove(&id)
    }

    /// Return a failed action to the queue without replacing a newer request.
    pub async fn restore(&self, request: ApprovalRequest) {
        if request.created_at.elapsed() > APPROVAL_TTL {
            return;
        }
        self.requests
            .lock()
            .await
            .entry(request.id)
            .or_insert(request);
    }

    pub async fn cleanup_expired(&self) -> usize {
        let mut requests = self.requests.lock().await;
        let previous_len = requests.len();
        requests.retain(|_, request| request.created_at.elapsed() <= APPROVAL_TTL);
        previous_len - requests.len()
    }
}

pub fn approval_buttons(prefix: &str, id: Uuid, disabled: bool) -> serenity::CreateActionRow {
    serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{prefix}_approve_{id}"))
            .label("Approve")
            .style(serenity::ButtonStyle::Success)
            .disabled(disabled),
        serenity::CreateButton::new(format!("{prefix}_reject_{id}"))
            .label("Reject")
            .style(serenity::ButtonStyle::Danger)
            .disabled(disabled),
    ])
}

pub fn has_role(member: &serenity::Member, role: serenity::RoleId) -> bool {
    member.roles.contains(&role)
}

pub fn has_any_role(member: &serenity::Member, roles: &[serenity::RoleId]) -> bool {
    roles.iter().any(|role| has_role(member, *role))
}

pub fn is_administrator(member: &serenity::Member) -> bool {
    member
        .permissions
        .is_some_and(serenity::Permissions::administrator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> ApprovalAction {
        ApprovalAction::Unban {
            target_id: serenity::UserId::new(2),
            reason: "test".into(),
        }
    }

    #[tokio::test]
    async fn an_approval_can_only_be_claimed_once() {
        let pending = PendingApprovals::default();
        let request = pending
            .create(
                serenity::UserId::new(1),
                "submitter".into(),
                serenity::GuildId::new(3),
                action(),
            )
            .await;

        assert!(pending.remove(request.id).await.is_some());
        assert!(pending.remove(request.id).await.is_none());
    }

    #[tokio::test]
    async fn a_failed_approval_can_be_restored() {
        let pending = PendingApprovals::default();
        let request = pending
            .create(
                serenity::UserId::new(1),
                "submitter".into(),
                serenity::GuildId::new(3),
                action(),
            )
            .await;
        let claimed = pending.remove(request.id).await.expect("request exists");

        pending.restore(claimed).await;

        assert!(pending.remove(request.id).await.is_some());
    }
}
