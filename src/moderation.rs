use std::{collections::HashMap, time::Duration};

use poise::serenity_prelude as serenity;
use tokio::sync::Mutex;
use uuid::Uuid;

const APPROVAL_TTL: Duration = Duration::from_hours(1);

#[derive(Clone, Debug)]
pub enum ApprovalAction {
    Banner {
        image_url: String,
    },
    Ban {
        target_id: serenity::UserId,
        reason: String,
        delete_message_days: u8,
    },
    MediaFrequency {
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
        self.requests
            .lock()
            .await
            .insert(request.id, request.clone());
        request
    }

    pub async fn get(&self, id: Uuid) -> Option<ApprovalRequest> {
        let mut requests = self.requests.lock().await;
        requests.retain(|_, request| request.created_at.elapsed() <= APPROVAL_TTL);
        requests.get(&id).cloned()
    }

    pub async fn remove(&self, id: Uuid) -> Option<ApprovalRequest> {
        self.requests.lock().await.remove(&id)
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
