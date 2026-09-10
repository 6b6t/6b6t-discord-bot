use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use chrono::{FixedOffset, NaiveDateTime, TimeZone as _, Utc};
use poise::serenity_prelude as serenity;
use sqlx::{MySql, Transaction};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    config::{self, EventChannels},
    database::Databases,
};

const APPLY_ID: &str = "events:apply";
const DRAFT_TTL: Duration = Duration::from_mins(30);
const REQUIRED_PLAYTIME_HOURS: i64 = 50;
// The player statistics service stores play_time in Minecraft ticks (20 per second).
const REQUIRED_PLAYTIME_TICKS: i64 = REQUIRED_PLAYTIME_HOURS * 60 * 60 * 20;
const DISCLAIMER: &str = "This event is organized by members of the 6b6t community and is not operated or endorsed by 6b6t. Participate at your own risk.";

#[derive(Clone)]
pub struct EventSubmissionService {
    channels: EventChannels,
    databases: Databases,
    test_user_id: Option<serenity::UserId>,
    initialized: std::sync::Arc<AtomicBool>,
    drafts: std::sync::Arc<Mutex<HashMap<Uuid, Draft>>>,
    menu_message: std::sync::Arc<Mutex<Option<serenity::MessageId>>>,
    menu_lock: std::sync::Arc<Mutex<()>>,
    review_lock: std::sync::Arc<Mutex<()>>,
    submission_lock: std::sync::Arc<Mutex<()>>,
    worker_lock: std::sync::Arc<Mutex<()>>,
    post_lock: std::sync::Arc<Mutex<()>>,
}

#[derive(Clone, Debug)]
struct Draft {
    owner: serenity::UserId,
    event_name: String,
    explanation: String,
    minecraft_username: String,
    discord_invite: String,
    promotion_url: String,
    expires_at: std::time::Instant,
}

#[derive(Clone, Debug)]
struct CompletedForm {
    event_name: String,
    explanation: String,
    minecraft_username: String,
    discord_invite: String,
    promotion_url: String,
    event_at: i64,
    event_time_input: String,
    join_instructions: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct Submission {
    id: u64,
    submitter_discord_id: String,
    minecraft_username: String,
    event_name: String,
    explanation: String,
    discord_invite: String,
    promotion_url: String,
    event_at: i64,
    event_time_input: String,
    join_instructions: String,
    status: String,
    denial_reason: Option<String>,
    review_message_id: Option<String>,
    event_message_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VoteResult {
    Added { approvals: u64, approved: bool },
    Duplicate,
    Resolved,
}

impl EventSubmissionService {
    pub fn new(
        channels: EventChannels,
        databases: Databases,
        test_user_id: Option<serenity::UserId>,
    ) -> Self {
        Self {
            channels,
            databases,
            test_user_id,
            initialized: std::sync::Arc::default(),
            drafts: std::sync::Arc::default(),
            menu_message: std::sync::Arc::default(),
            menu_lock: std::sync::Arc::default(),
            review_lock: std::sync::Arc::default(),
            submission_lock: std::sync::Arc::default(),
            worker_lock: std::sync::Arc::default(),
            post_lock: std::sync::Arc::default(),
        }
    }

    pub async fn ready(&self, ctx: &serenity::Context) -> Result<()> {
        self.validate_discord_setup(ctx).await?;
        sqlx::query("UPDATE event_submissions SET status = 'approved' WHERE status = 'posting'")
            .execute(&self.databases.link)
            .await
            .context("failed to recover interrupted event posts")?;
        self.ensure_menu(ctx).await?;
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    async fn validate_discord_setup(&self, ctx: &serenity::Context) -> Result<()> {
        let events = self.channels.events.to_channel(ctx).await?;
        let serenity::Channel::Guild(events) = events else {
            bail!("EVENTS_CHANNEL_ID is not a guild channel");
        };
        if events.guild_id != config::GUILD_ID {
            bail!("the events channel must belong to the configured server");
        }
        if self.channels.events == self.channels.review
            || self.channels.events == self.channels.logs
            || self.channels.review == self.channels.logs
        {
            bail!("events, review, and audit logs must use separate channels");
        }
        if events.kind != serenity::ChannelType::News {
            bail!("EVENTS_CHANNEL_ID must be an Announcement channel");
        }
        for (name, id) in [
            ("EVENTS_REVIEW_CHANNEL_ID", self.channels.review),
            ("EVENTS_LOG_CHANNEL_ID", self.channels.logs),
        ] {
            let channel = id.to_channel(ctx).await?;
            if !matches!(channel, serenity::Channel::Guild(channel) if channel.guild_id == config::GUILD_ID)
            {
                bail!("{name} must be a channel in the configured server");
            }
        }
        let roles = config::GUILD_ID.roles(ctx).await?;
        for (name, id) in [
            ("Events", config::EVENTS_ROLE_ID),
            ("Linked", config::LINKED_ROLE_ID),
            ("Terminator", config::TERMINATOR_ROLE_ID),
            ("Marketer", config::MARKETER_ROLE_ID),
        ] {
            if !roles.contains_key(&id) {
                bail!("configured {name} role ({id}) does not exist");
            }
        }
        Ok(())
    }

    pub async fn handle_component(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ComponentInteraction,
    ) -> Result<bool> {
        let custom_id = interaction.data.custom_id.as_str();
        if !custom_id.starts_with("events:") {
            return Ok(false);
        }
        if !self.initialized.load(Ordering::Acquire) {
            component_reply(
                ctx,
                interaction,
                "The event system is still starting. Please try again shortly.",
            )
            .await?;
            return Ok(true);
        }
        if custom_id == APPLY_ID {
            self.apply(ctx, interaction).await?;
        } else if let Some(value) = custom_id.strip_prefix("events:continue:") {
            self.continue_application(ctx, interaction, value).await?;
        } else if let Some(value) = custom_id.strip_prefix("events:approve:") {
            self.approve(ctx, interaction, parse_event_id(value)?)
                .await?;
        } else if let Some(value) = custom_id.strip_prefix("events:deny:") {
            self.open_denial(ctx, interaction, parse_event_id(value)?)
                .await?;
        }
        Ok(true)
    }

    pub async fn handle_modal(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ModalInteraction,
    ) -> Result<bool> {
        let custom_id = interaction.data.custom_id.as_str();
        if !custom_id.starts_with("events:") {
            return Ok(false);
        }
        if !self.initialized.load(Ordering::Acquire) {
            modal_reply(
                ctx,
                interaction,
                "The event system is still starting. Please try again shortly.",
                Vec::new(),
            )
            .await?;
            return Ok(true);
        }
        let result = if custom_id == "events:form1" {
            self.first_form(ctx, interaction).await
        } else if let Some(value) = custom_id.strip_prefix("events:form2:") {
            self.second_form(ctx, interaction, value).await
        } else if let Some(value) = custom_id.strip_prefix("events:deny-modal:") {
            self.deny(ctx, interaction, parse_event_id(value)?).await
        } else {
            Ok(())
        };
        if let Err(error) = result {
            tracing::warn!(%error, user_id = %interaction.user.id, "event modal could not be processed");
            modal_reply(
                ctx,
                interaction,
                format!("I couldn't process that form: {error}"),
                Vec::new(),
            )
            .await?;
        }
        Ok(true)
    }

    async fn apply(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ComponentInteraction,
    ) -> Result<()> {
        if interaction.channel_id != self.channels.events {
            return component_reply(ctx, interaction, "This Apply button is no longer valid.")
                .await;
        }
        let member = interaction
            .member
            .as_ref()
            .context("Apply used outside a guild")?;
        if !member.roles.contains(&config::LINKED_ROLE_ID) {
            return component_reply(
                ctx,
                interaction,
                format!("You need the <@&{}> role to apply.", config::LINKED_ROLE_ID),
            )
            .await;
        }
        self.cleanup_drafts().await;
        if self
            .drafts
            .lock()
            .await
            .values()
            .any(|draft| draft.owner == interaction.user.id)
            || self.has_pending(interaction.user.id).await?
        {
            return component_reply(
                ctx,
                interaction,
                "You already have an unfinished or pending event application.",
            )
            .await;
        }
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Modal(first_modal()),
            )
            .await?;
        Ok(())
    }

    async fn first_form(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ModalInteraction,
    ) -> Result<()> {
        let fields = modal_fields(interaction);
        let draft = Draft {
            owner: interaction.user.id,
            event_name: required_field(&fields, "event_name", 100)?,
            explanation: required_field(&fields, "explanation", 1_000)?,
            minecraft_username: minecraft_name(&required_field(
                &fields,
                "minecraft_username",
                16,
            )?)?,
            discord_invite: validate_invite(&required_field(&fields, "discord_invite", 512)?)?,
            promotion_url: validate_promotion(&required_field(&fields, "promotion_url", 512)?)?,
            expires_at: std::time::Instant::now() + DRAFT_TTL,
        };
        self.cleanup_drafts().await;
        if self.has_pending(interaction.user.id).await? {
            return modal_reply(
                ctx,
                interaction,
                "You already have a pending event application.",
                Vec::new(),
            )
            .await;
        }
        let id = Uuid::new_v4();
        let mut drafts = self.drafts.lock().await;
        if drafts
            .values()
            .any(|existing| existing.owner == interaction.user.id)
        {
            drop(drafts);
            return modal_reply(
                ctx,
                interaction,
                "You already have an unfinished event application.",
                Vec::new(),
            )
            .await;
        }
        drafts.insert(id, draft);
        drop(drafts);
        modal_reply(
            ctx,
            interaction,
            "Part one is saved for 30 minutes. Continue to enter the date and joining instructions.",
            continue_button(id),
        )
        .await
    }

    async fn continue_application(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ComponentInteraction,
        value: &str,
    ) -> Result<()> {
        let id = Uuid::parse_str(value).context("invalid event draft ID")?;
        self.cleanup_drafts().await;
        let drafts = self.drafts.lock().await;
        let Some(draft) = drafts.get(&id) else {
            return component_reply(ctx, interaction, "This draft expired. Please start again.")
                .await;
        };
        if draft.owner != interaction.user.id {
            return component_reply(ctx, interaction, "This draft belongs to another user.").await;
        }
        drop(drafts);
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Modal(second_modal(id)),
            )
            .await?;
        Ok(())
    }

    async fn second_form(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ModalInteraction,
        value: &str,
    ) -> Result<()> {
        let draft_id = Uuid::parse_str(value).context("invalid event draft ID")?;
        self.cleanup_drafts().await;
        let Some(draft) = self.drafts.lock().await.get(&draft_id).cloned() else {
            return modal_reply(
                ctx,
                interaction,
                "This draft expired. Please start again.",
                Vec::new(),
            )
            .await;
        };
        if draft.owner != interaction.user.id {
            return modal_reply(
                ctx,
                interaction,
                "This draft belongs to another user.",
                Vec::new(),
            )
            .await;
        }
        let fields = modal_fields(interaction);
        let validated = (|| -> Result<_> {
            let time_input = required_field(&fields, "event_time", 64)?;
            let event_at = parse_event_time(&time_input)?;
            let instructions = required_field(&fields, "join_instructions", 1_000)?;
            Ok((time_input, event_at, instructions))
        })();
        let (time_input, event_at, instructions) = match validated {
            Ok(values) => values,
            Err(error) => return modal_reply(
                ctx,
                interaction,
                format!(
                    "Please check your details: {error}. Your saved application is still available."
                ),
                continue_button(draft_id),
            )
            .await,
        };
        let form = CompletedForm {
            event_name: draft.event_name,
            explanation: draft.explanation,
            minecraft_username: draft.minecraft_username,
            discord_invite: draft.discord_invite,
            promotion_url: draft.promotion_url,
            event_at,
            event_time_input: time_input,
            join_instructions: instructions,
        };
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Defer(
                    serenity::CreateInteractionResponseMessage::new().ephemeral(true),
                ),
            )
            .await?;
        let outcome = self.complete_submission(ctx, interaction, form).await;
        let content = match outcome {
            Ok(content) => {
                self.drafts.lock().await.remove(&draft_id);
                content
            }
            Err(error) => {
                tracing::error!(?error, user_id = %interaction.user.id, "event submission failed");
                interaction.edit_response(ctx, serenity::EditInteractionResponse::new()
                    .content("We could not submit your application. Your saved details are available until the draft expires. Please try again.")
                    .components(continue_button(draft_id))).await?;
                return Ok(());
            }
        };
        interaction
            .edit_response(
                ctx,
                serenity::EditInteractionResponse::new().content(content),
            )
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Keep validation, persistence, and the submission receipt together.
    async fn complete_submission(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ModalInteraction,
        form: CompletedForm,
    ) -> Result<String> {
        let _guard = self.submission_lock.lock().await;
        if form.event_at <= Utc::now().timestamp() {
            bail!("the event start time has passed; please choose a future date");
        }
        let existing: Option<u64> = sqlx::query_scalar("SELECT id FROM event_submissions WHERE submitter_discord_id = ? AND event_at > UNIX_TIMESTAMP() AND status IN ('pending', 'approved', 'posting') AND (status = 'pending' OR event_message_id IS NULL) ORDER BY id DESC LIMIT 1")
            .bind(interaction.user.id.to_string()).fetch_optional(&self.databases.link).await?;
        if let Some(id) = existing {
            return Ok(format!(
                "Application EVT-{id} is already saved and awaiting review or posting. You do not need to submit it again."
            ));
        }
        if !interaction
            .member
            .as_ref()
            .is_some_and(|member| member.roles.contains(&config::LINKED_ROLE_ID))
        {
            bail!("you no longer have the Linked role");
        }
        let mapping = self
            .databases
            .mapping_for_discord(&interaction.user.id.to_string())
            .await?
            .context("your linked Minecraft account could not be found")?;
        let player = self
            .databases
            .player_info(&mapping.uuid)
            .await?
            .context("your linked Minecraft player could not be found")?;
        let (denial, playtime_bypassed) = self
            .submission_denial(
                interaction.user.id,
                &player.name,
                &form.minecraft_username,
                &mapping.uuid,
            )
            .await?;
        let status = if denial.is_some() {
            "auto_denied"
        } else {
            "pending"
        };
        let reason = denial.map(|(reason, _)| reason);
        let result = sqlx::query(
            "INSERT INTO event_submissions (submitter_discord_id, linked_uuid, minecraft_username, event_name, explanation, discord_invite, promotion_url, event_at, event_time_input, join_instructions, status, denial_reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(interaction.user.id.to_string())
        .bind(&mapping.uuid)
        .bind(&form.minecraft_username)
        .bind(&form.event_name)
        .bind(&form.explanation)
        .bind(&form.discord_invite)
        .bind(&form.promotion_url)
        .bind(form.event_at)
        .bind(&form.event_time_input)
        .bind(&form.join_instructions)
        .bind(status)
        .bind(reason)
        .execute(&self.databases.link)
        .await
        .context("failed to save event application")?;
        let id = result.last_insert_id();
        let submission = self
            .submission(id)
            .await?
            .context("saved event disappeared")?;
        self.log(
            ctx,
            "Event submitted",
            format!(
                "EVT-{id}\nSubmitter: <@{}>\nStatus: {status}",
                interaction.user.id
            ),
            0x00FF_F11A,
        )
        .await;
        if playtime_bypassed {
            self.log_test_bypass(ctx, id, interaction.user.id).await;
        }
        if let Some((reason, category)) = denial {
            self.log(
                ctx,
                "Event automatically denied",
                format!(
                    "EVT-{id}\nSubmitter: <@{}>\nReason category: {category}",
                    interaction.user.id
                ),
                0x00ED_4245,
            )
            .await;
            return Ok(format!(
                "Application EVT-{id} does not meet the requirements: {reason}. We will attempt to send a copy to your direct messages."
            ));
        }
        match self.deliver_review(ctx, &submission).await {
            Ok(()) => Ok(format!("EVT-{id} was submitted for staff review.")),
            Err(error) => {
                tracing::error!(%error, event_id = id, "event review delivery failed; worker will retry");
                Ok(format!(
                    "EVT-{id} was saved. Staff delivery is temporarily delayed and will retry automatically."
                ))
            }
        }
    }

    async fn approve(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ComponentInteraction,
        id: u64,
    ) -> Result<()> {
        let member = interaction
            .member
            .as_ref()
            .context("vote used outside a guild")?;
        if !is_reviewer(member) {
            return component_reply(ctx, interaction, "Only Terminators and Marketers can vote.")
                .await;
        }
        let Some(submission) = self.submission(id).await? else {
            return component_reply(ctx, interaction, "This event no longer exists.").await;
        };
        if submission.submitter_discord_id == interaction.user.id.to_string() {
            return component_reply(ctx, interaction, "You cannot vote on your own event.").await;
        }
        let result = self.record_vote(id, interaction.user.id).await?;
        match result {
            VoteResult::Duplicate => {
                return component_reply(ctx, interaction, "You already approved this event.").await;
            }
            VoteResult::Resolved => {
                return component_reply(ctx, interaction, "This event has already been resolved.")
                    .await;
            }
            VoteResult::Added {
                approvals,
                approved,
            } => {
                interaction
                    .create_response(ctx, serenity::CreateInteractionResponse::Acknowledge)
                    .await?;
                let voters = self.voters(id).await?;
                if let Err(error) = self
                    .update_review(ctx, interaction.message.id, &submission, &voters, approved)
                    .await
                {
                    tracing::warn!(%error, event_id = id, "failed to update event review");
                }
                self.log(
                    ctx,
                    "Event approval vote",
                    format!(
                        "EVT-{id}\nVoter: <@{}>\nApprovals: {approvals}/3",
                        interaction.user.id
                    ),
                    0x0057_F287,
                )
                .await;
                if approved {
                    self.log(
                        ctx,
                        "Event approved",
                        format!(
                            "EVT-{id}\nSubmitter: <@{}>",
                            submission.submitter_discord_id
                        ),
                        0x0057_F287,
                    )
                    .await;
                    if let Err(error) = self.post_approved(ctx, id).await {
                        tracing::error!(%error, event_id = id, "approved event post failed; worker will retry");
                    }
                }
            }
        }
        Ok(())
    }

    async fn open_denial(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ComponentInteraction,
        id: u64,
    ) -> Result<()> {
        let member = interaction
            .member
            .as_ref()
            .context("denial used outside a guild")?;
        if !is_reviewer(member) {
            return component_reply(
                ctx,
                interaction,
                "Only Terminators and Marketers can deny events.",
            )
            .await;
        }
        let Some(submission) = self.submission(id).await? else {
            return component_reply(ctx, interaction, "This event no longer exists.").await;
        };
        if submission.submitter_discord_id == interaction.user.id.to_string() {
            return component_reply(ctx, interaction, "You cannot vote on your own event.").await;
        }
        if submission.status != "pending" || submission.event_at <= Utc::now().timestamp() {
            return component_reply(ctx, interaction, "This event has already been resolved.")
                .await;
        }
        let input =
            serenity::CreateInputText::new(serenity::InputTextStyle::Paragraph, "Reason", "reason")
                .min_length(3)
                .max_length(1_000)
                .required(true);
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Modal(
                    serenity::CreateModal::new(
                        format!("events:deny-modal:{id}"),
                        format!("Deny EVT-{id}"),
                    )
                    .components(vec![serenity::CreateActionRow::InputText(input)]),
                ),
            )
            .await?;
        Ok(())
    }

    async fn deny(
        &self,
        ctx: &serenity::Context,
        interaction: &serenity::ModalInteraction,
        id: u64,
    ) -> Result<()> {
        let member = interaction
            .member
            .as_ref()
            .context("denial used outside a guild")?;
        if !is_reviewer(member) {
            return modal_reply(
                ctx,
                interaction,
                "Only Terminators and Marketers can deny events.",
                Vec::new(),
            )
            .await;
        }
        let reason = required_field(&modal_fields(interaction), "reason", 1_000)?;
        let mut tx = self.databases.link.begin().await?;
        let Some(submission) = locked_submission(&mut tx, id).await? else {
            tx.rollback().await?;
            return modal_reply(ctx, interaction, "This event no longer exists.", Vec::new()).await;
        };
        if submission.submitter_discord_id == interaction.user.id.to_string() {
            tx.rollback().await?;
            return modal_reply(
                ctx,
                interaction,
                "You cannot vote on your own event.",
                Vec::new(),
            )
            .await;
        }
        if submission.status != "pending" || submission.event_at <= Utc::now().timestamp() {
            tx.rollback().await?;
            return modal_reply(
                ctx,
                interaction,
                "This event has already been resolved.",
                Vec::new(),
            )
            .await;
        }
        sqlx::query(
            "UPDATE event_submissions SET status = 'denied', denial_reason = ? WHERE id = ?",
        )
        .bind(&reason)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.log(
            ctx,
            "Event denied",
            format!(
                "EVT-{id}\nReviewer: <@{}>\nReason: {reason}",
                interaction.user.id
            ),
            0x00ED_4245,
        )
        .await;
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .content(format!("EVT-{id} was denied."))
                        .ephemeral(true),
                ),
            )
            .await?;
        if let Some(message_id) = submission
            .review_message_id
            .as_deref()
            .and_then(parse_message_id)
        {
            let voters = self.voters(id).await?;
            if let Err(error) = self
                .update_review(ctx, message_id, &submission, &voters, true)
                .await
            {
                tracing::warn!(%error, event_id = id, "failed to update denied event review");
            }
        }
        Ok(())
    }

    async fn record_vote(&self, id: u64, voter: serenity::UserId) -> Result<VoteResult> {
        let mut tx = self.databases.link.begin().await?;
        let Some(submission) = locked_submission(&mut tx, id).await? else {
            tx.rollback().await?;
            return Ok(VoteResult::Resolved);
        };
        if submission.status != "pending" || submission.event_at <= Utc::now().timestamp() {
            tx.rollback().await?;
            return Ok(VoteResult::Resolved);
        }
        let inserted = sqlx::query(
            "INSERT IGNORE INTO event_votes (event_id, voter_discord_id) VALUES (?, ?)",
        )
        .bind(id)
        .bind(voter.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 0 {
            tx.rollback().await?;
            return Ok(VoteResult::Duplicate);
        }
        let approvals: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM event_votes WHERE event_id = ?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        let approved = approvals >= 3;
        if approved {
            sqlx::query("UPDATE event_submissions SET status = 'approved' WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(VoteResult::Added {
            approvals: u64::try_from(approvals).context("event approval count was negative")?,
            approved,
        })
    }

    pub async fn on_message(&self, ctx: &serenity::Context, message: &serenity::Message) {
        if !self.initialized.load(Ordering::Acquire) || message.channel_id != self.channels.events {
            return;
        }
        if is_menu_message(ctx, message) {
            *self.menu_message.lock().await = Some(message.id);
            return;
        }
        if let Err(error) = self.ensure_menu(ctx).await {
            tracing::error!(%error, "failed to keep event application menu at channel bottom");
        }
    }

    pub async fn on_delete(
        &self,
        ctx: &serenity::Context,
        channel_id: serenity::ChannelId,
        message_id: serenity::MessageId,
    ) {
        if !self.initialized.load(Ordering::Acquire) || channel_id != self.channels.events {
            return;
        }
        if *self.menu_message.lock().await == Some(message_id) {
            *self.menu_message.lock().await = None;
            if let Err(error) = self.ensure_menu(ctx).await {
                tracing::error!(%error, "failed to restore deleted event menu");
            }
            return;
        }
        match sqlx::query_scalar::<_, u64>(
            "SELECT id FROM event_submissions WHERE event_message_id = ? AND status = 'approved' LIMIT 1",
        )
        .bind(message_id.to_string())
        .fetch_optional(&self.databases.link)
        .await
        {
            Ok(Some(id)) => {
                if let Err(error) = sqlx::query(
                    "UPDATE event_submissions SET status = 'deleted', deleted_at = UTC_TIMESTAMP() WHERE id = ? AND status = 'approved'",
                )
                .bind(id)
                .execute(&self.databases.link)
                .await
                {
                    tracing::error!(%error, event_id = id, "failed to record event deletion");
                    return;
                }
                let actor = self
                    .deletion_actor(
                        ctx,
                        serenity::audit_log::MessageAction::Delete,
                        Some(message_id),
                    )
                    .await;
                self.log(
                    ctx,
                    "Approved event deleted",
                    format!("EVT-{id}\nDeleted by: {actor}"),
                    0x00ED_4245,
                )
                .await;
            }
            Ok(None) => {}
            Err(error) => tracing::error!(%error, "failed to correlate deleted event message"),
        }
    }

    pub async fn on_bulk_delete(
        &self,
        ctx: &serenity::Context,
        channel_id: serenity::ChannelId,
        message_ids: &[serenity::MessageId],
    ) {
        if !self.initialized.load(Ordering::Acquire) || channel_id != self.channels.events {
            return;
        }
        let actor = self
            .deletion_actor(ctx, serenity::audit_log::MessageAction::BulkDelete, None)
            .await;
        for message_id in message_ids {
            if *self.menu_message.lock().await == Some(*message_id) {
                *self.menu_message.lock().await = None;
                continue;
            }
            match sqlx::query_scalar::<_, u64>(
                "SELECT id FROM event_submissions WHERE event_message_id = ? AND status = 'approved' LIMIT 1",
            )
            .bind(message_id.to_string())
            .fetch_optional(&self.databases.link)
            .await
            {
                Ok(Some(id)) => {
                    match sqlx::query("UPDATE event_submissions SET status = 'deleted', deleted_at = UTC_TIMESTAMP() WHERE id = ? AND status = 'approved'")
                        .bind(id)
                        .execute(&self.databases.link)
                        .await
                    {
                        Ok(result) if result.rows_affected() > 0 => {
                            self.log(
                                ctx,
                                "Approved event deleted",
                                format!("EVT-{id}\nDeleted by: {actor}"),
                                0x00ED_4245,
                            )
                            .await;
                        }
                        Ok(_) => {}
                        Err(error) => tracing::error!(%error, event_id = id, "failed to record bulk event deletion"),
                    }
                }
                Ok(None) => {}
                Err(error) => tracing::error!(%error, "failed to correlate bulk-deleted event message"),
            }
        }
        let menu_missing = self.menu_message.lock().await.is_none();
        if menu_missing && let Err(error) = self.ensure_menu(ctx).await {
            tracing::error!(%error, "failed to restore bulk-deleted event menu");
        }
    }

    async fn deletion_actor(
        &self,
        ctx: &serenity::Context,
        action: serenity::audit_log::MessageAction,
        message_id: Option<serenity::MessageId>,
    ) -> String {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let action = serenity::audit_log::Action::Message(action);
        let Ok(logs) = config::GUILD_ID
            .audit_logs(ctx, Some(action), None, None, Some(10))
            .await
        else {
            return "Attribution unavailable".into();
        };
        let now = Utc::now().timestamp();
        logs.entries
            .into_iter()
            .find(|entry| {
                entry.options.as_ref().is_some_and(|options| {
                    options.channel_id == Some(self.channels.events)
                        && message_id.is_none_or(|expected| {
                            options.message_id.is_none_or(|id| id == expected)
                        })
                }) && now - entry.id.created_at().unix_timestamp() <= 20
                    && entry
                        .target_id
                        .is_none_or(|id| id.get() == ctx.cache.current_user().id.get())
            })
            .map_or_else(
                || "Attribution unavailable".into(),
                |entry| format!("<@{}>", entry.user_id),
            )
    }

    pub async fn poll(&self, ctx: &serenity::Context) {
        if !self.initialized.load(Ordering::Acquire) {
            return;
        }
        let Ok(_guard) = self.worker_lock.try_lock() else {
            return;
        };
        if let Err(error) = sqlx::query("UPDATE event_submissions SET status = 'expired' WHERE status = 'pending' AND event_at <= UNIX_TIMESTAMP()")
            .execute(&self.databases.link).await {
            tracing::error!(%error, "failed to expire pending events");
        }
        let approved = sqlx::query_scalar::<_, u64>(
            "SELECT id FROM event_submissions WHERE status IN ('approved', 'posting') AND event_message_id IS NULL ORDER BY id LIMIT 20",
        )
        .fetch_all(&self.databases.link)
        .await;
        match approved {
            Ok(ids) => {
                for id in ids {
                    if let Err(error) = self.post_approved(ctx, id).await {
                        tracing::error!(%error, event_id = id, "approved event retry failed");
                    }
                }
            }
            Err(error) => tracing::error!(%error, "failed to load approved events"),
        }
        let undelivered_reviews = sqlx::query_as::<_, Submission>(
            "SELECT id, submitter_discord_id, minecraft_username, event_name, explanation, discord_invite, promotion_url, event_at, event_time_input, join_instructions, status, denial_reason, review_message_id, event_message_id FROM event_submissions WHERE status = 'pending' ORDER BY review_message_id IS NOT NULL, updated_at, id LIMIT 20",
        )
        .fetch_all(&self.databases.link)
        .await;
        match undelivered_reviews {
            Ok(submissions) => {
                for submission in submissions {
                    if let Err(error) = self.deliver_review(ctx, &submission).await {
                        tracing::error!(%error, event_id = submission.id, "event review delivery retry failed");
                    }
                }
            }
            Err(error) => tracing::error!(%error, "failed to load undelivered event reviews"),
        }
        let due = sqlx::query_as::<_, Submission>(
            "SELECT id, submitter_discord_id, minecraft_username, event_name, explanation, discord_invite, promotion_url, event_at, event_time_input, join_instructions, status, denial_reason, review_message_id, event_message_id FROM event_submissions WHERE status = 'approved' AND event_message_id IS NOT NULL AND publish_at <= UTC_TIMESTAMP() AND published_at IS NULL ORDER BY publish_at LIMIT 20",
        )
        .fetch_all(&self.databases.link)
        .await;
        match due {
            Ok(submissions) => {
                for submission in submissions {
                    if let Err(error) = self.publish(ctx, &submission).await {
                        tracing::error!(%error, event_id = submission.id, "event publication failed; will retry");
                    }
                }
            }
            Err(error) => tracing::error!(%error, "failed to load due event publications"),
        }
        if let Err(error) = self.notify_resolutions(ctx).await {
            tracing::error!(%error, "event resolution notification retry failed");
        }
    }

    async fn expire(&self, id: u64) -> Result<()> {
        sqlx::query("UPDATE event_submissions SET status = 'expired' WHERE id = ? AND status IN ('pending', 'approved', 'posting')")
            .bind(id).execute(&self.databases.link).await?;
        Ok(())
    }

    async fn notify_resolutions(&self, ctx: &serenity::Context) -> Result<()> {
        let submissions = sqlx::query_as::<_, Submission>(
            "SELECT e.id, e.submitter_discord_id, e.minecraft_username, e.event_name, e.explanation, e.discord_invite, e.promotion_url, e.event_at, e.event_time_input, e.join_instructions, e.status, e.denial_reason, e.review_message_id, e.event_message_id FROM event_submissions e LEFT JOIN event_notifications n ON n.event_id = e.id AND n.status = e.status WHERE e.status IN ('approved', 'denied', 'auto_denied', 'expired') AND (e.status != 'approved' OR e.event_message_id IS NOT NULL) AND (n.event_id IS NULL OR ((n.delivered = 0 OR n.review_updated = 0) AND n.updated_at <= DATE_SUB(UTC_TIMESTAMP(), INTERVAL 1 HOUR))) ORDER BY n.updated_at, e.id LIMIT 20"
        ).fetch_all(&self.databases.link).await?;
        for submission in submissions {
            let reason = submission
                .denial_reason
                .as_deref()
                .unwrap_or("The event start time has passed.");
            let previous: Option<(bool, bool)> = sqlx::query_as("SELECT delivered, review_updated FROM event_notifications WHERE event_id = ? AND status = ?")
                .bind(submission.id).bind(&submission.status).fetch_optional(&self.databases.link).await?;
            let (mut delivered, mut review_updated) = previous.unwrap_or_default();
            if !delivered {
                delivered = self.dm_resolution(ctx, &submission, reason).await;
            }
            // Record the DM outcome before attempting the independent review update.
            sqlx::query("INSERT INTO event_notifications (event_id, status, delivered, updated_at) VALUES (?, ?, ?, UTC_TIMESTAMP()) ON DUPLICATE KEY UPDATE delivered = VALUES(delivered), updated_at = UTC_TIMESTAMP()")
                .bind(submission.id).bind(&submission.status).bind(delivered).execute(&self.databases.link).await?;
            if !review_updated {
                review_updated = if let Some(id) = submission
                    .review_message_id
                    .as_deref()
                    .and_then(parse_message_id)
                {
                    let voters = self.voters(submission.id).await?;
                    match self
                        .update_review(ctx, id, &submission, &voters, true)
                        .await
                    {
                        Ok(()) => true,
                        Err(error)
                            if error
                                .downcast_ref::<serenity::Error>()
                                .is_some_and(is_unknown_message) =>
                        {
                            true
                        }
                        Err(error) => {
                            tracing::warn!(%error, event_id = submission.id, "failed to update resolved event review");
                            false
                        }
                    }
                } else {
                    true
                };
            }
            sqlx::query("INSERT INTO event_notifications (event_id, status, delivered, review_updated, updated_at) VALUES (?, ?, ?, ?, UTC_TIMESTAMP()) ON DUPLICATE KEY UPDATE delivered = VALUES(delivered), review_updated = VALUES(review_updated), updated_at = UTC_TIMESTAMP()")
                .bind(submission.id).bind(&submission.status).bind(delivered).bind(review_updated).execute(&self.databases.link).await?;
        }
        Ok(())
    }

    async fn post_approved(&self, ctx: &serenity::Context, id: u64) -> Result<()> {
        // ponytail: one process serializes event posts; use database leases before running replicas.
        let _guard = self.post_lock.lock().await;
        let Some(submission) = self.submission(id).await? else {
            return Ok(());
        };
        if !matches!(submission.status.as_str(), "approved" | "posting")
            || submission.event_message_id.is_some()
        {
            return Ok(());
        }
        let recent = self
            .channels
            .events
            .messages(ctx, serenity::GetMessages::new().limit(100))
            .await?;
        if let Some(message) = recent.iter().find(|message| {
            message.author.id == ctx.cache.current_user().id
                && message.embeds.iter().any(|embed| {
                    embed
                        .footer
                        .as_ref()
                        .is_some_and(|footer| footer.text == format!("EVT-{id}"))
                })
        }) {
            self.attach_post(id, message.id, message.timestamp.unix_timestamp())
                .await?;
            return Ok(());
        }
        if submission.event_at <= Utc::now().timestamp() {
            self.expire(id).await?;
            return Ok(());
        }
        let claimed = sqlx::query(
            "UPDATE event_submissions SET status = 'posting' WHERE id = ? AND status IN ('approved', 'posting') AND event_message_id IS NULL",
        )
        .bind(id)
        .execute(&self.databases.link)
        .await?
        .rows_affected();
        if claimed == 0 && submission.status != "posting" {
            return Ok(());
        }
        let message = self
            .channels
            .events
            .send_message(ctx, approved_message(&submission))
            .await;
        match message {
            Ok(message) => {
                self.attach_post(id, message.id, message.timestamp.unix_timestamp())
                    .await?;
                self.log(
                    ctx,
                    "Approved event posted",
                    format!("EVT-{id}\nMessage: {}", message.link()),
                    0x0057_F287,
                )
                .await;
                self.ensure_menu(ctx).await?;
                Ok(())
            }
            Err(error) => {
                sqlx::query("UPDATE event_submissions SET status = 'approved' WHERE id = ? AND status = 'posting'")
                    .bind(id)
                    .execute(&self.databases.link)
                    .await?;
                Err(error.into())
            }
        }
    }

    async fn attach_post(
        &self,
        id: u64,
        message_id: serenity::MessageId,
        posted_at: i64,
    ) -> Result<()> {
        sqlx::query("UPDATE event_submissions SET status = 'approved', event_message_id = ?, publish_at = DATE_ADD(TIMESTAMPADD(SECOND, ?, '1970-01-01 00:00:00'), INTERVAL 120 MINUTE) WHERE id = ? AND event_message_id IS NULL")
            .bind(message_id.to_string())
            .bind(posted_at)
            .bind(id)
            .execute(&self.databases.link)
            .await?;
        Ok(())
    }

    async fn publish(&self, ctx: &serenity::Context, submission: &Submission) -> Result<()> {
        if submission.event_at <= Utc::now().timestamp() {
            self.expire(submission.id).await?;
            return Ok(());
        }
        let message_id = submission
            .event_message_id
            .as_deref()
            .and_then(parse_message_id)
            .context("approved event has an invalid message ID")?;
        let message = self.channels.events.message(ctx, message_id).await;
        let message = match message {
            Ok(message) => message,
            Err(error) if is_unknown_message(&error) => {
                tracing::warn!(%error, event_id = submission.id, "approved event message no longer exists");
                sqlx::query("UPDATE event_submissions SET status = 'deleted', deleted_at = UTC_TIMESTAMP() WHERE id = ?")
                    .bind(submission.id)
                    .execute(&self.databases.link)
                    .await?;
                self.log(
                    ctx,
                    "Approved event deleted",
                    format!(
                        "EVT-{}\nDeleted by: Attribution unavailable (deleted while bot was offline)",
                        submission.id
                    ),
                    0x00ED_4245,
                )
                .await;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if !message
            .flags
            .is_some_and(|flags| flags.contains(serenity::MessageFlags::CROSSPOSTED))
        {
            message.crosspost(ctx).await?;
        }
        sqlx::query("UPDATE event_submissions SET published_at = UTC_TIMESTAMP() WHERE id = ? AND published_at IS NULL")
            .bind(submission.id)
            .execute(&self.databases.link)
            .await?;
        self.log(
            ctx,
            "Event published",
            format!("EVT-{}\nMessage: {}", submission.id, message.link()),
            0x0057_F287,
        )
        .await;
        Ok(())
    }

    async fn ensure_menu(&self, ctx: &serenity::Context) -> Result<()> {
        let _guard = self.menu_lock.lock().await;
        let cached_menu = *self.menu_message.lock().await;
        if let Some(id) = cached_menu
            && self.channels.events.message(ctx, id).await.is_ok()
        {
            let latest = self
                .channels
                .events
                .messages(ctx, serenity::GetMessages::new().limit(1))
                .await?;
            if latest.first().is_some_and(|message| message.id == id) {
                self.channels
                    .events
                    .edit_message(ctx, id, serenity::EditMessage::new().embed(menu_embed()))
                    .await?;
                return Ok(());
            }
            *self.menu_message.lock().await = None;
            if let Err(error) = self.channels.events.delete_message(ctx, id).await {
                tracing::warn!(%error, message_id = %id, "failed to remove old event application menu");
            }
        }
        let messages = self
            .channels
            .events
            .messages(ctx, serenity::GetMessages::new().limit(100))
            .await?;
        for message in messages
            .iter()
            .filter(|message| is_menu_message(ctx, message))
        {
            if let Err(error) = message.delete(ctx).await {
                tracing::warn!(%error, message_id = %message.id, "failed to remove duplicate event menu");
            }
        }
        let message = self
            .channels
            .events
            .send_message(ctx, menu_message())
            .await?;
        *self.menu_message.lock().await = Some(message.id);
        Ok(())
    }

    async fn send_review(
        &self,
        ctx: &serenity::Context,
        submission: &Submission,
        voters: &[serenity::UserId],
    ) -> Result<serenity::Message> {
        self.channels
            .review
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .nonce(serenity::Nonce::String(format!("r{}", submission.id)))
                    .enforce_nonce(true)
                    .embed(review_embed(submission, voters, false))
                    .components(review_buttons(submission.id, false))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .map_err(Into::into)
    }

    async fn deliver_review(&self, ctx: &serenity::Context, submission: &Submission) -> Result<()> {
        let _guard = self.review_lock.lock().await;
        let fresh = self
            .submission(submission.id)
            .await?
            .context("event submission disappeared before review delivery")?;
        if fresh.status != "pending" {
            return Ok(());
        }
        if let Some(message_id) = fresh
            .review_message_id
            .as_deref()
            .and_then(parse_message_id)
        {
            match self.channels.review.message(ctx, message_id).await {
                Ok(_) => {
                    self.update_review(
                        ctx,
                        message_id,
                        &fresh,
                        &self.voters(fresh.id).await?,
                        false,
                    )
                    .await?;
                    sqlx::query(
                        "UPDATE event_submissions SET updated_at = UTC_TIMESTAMP() WHERE id = ?",
                    )
                    .bind(fresh.id)
                    .execute(&self.databases.link)
                    .await?;
                    return Ok(());
                }
                Err(error) if is_unknown_message(&error) => {
                    sqlx::query("UPDATE event_submissions SET review_message_id = NULL WHERE id = ? AND status = 'pending'")
                        .bind(fresh.id).execute(&self.databases.link).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let marker = format!("EVT-{}", fresh.id);
        let recent = self
            .channels
            .review
            .messages(ctx, serenity::GetMessages::new().limit(100))
            .await?;
        let message_id = if let Some(message) = recent.iter().find(|message| {
            message.author.id == ctx.cache.current_user().id
                && message.embeds.iter().any(|embed| {
                    embed
                        .footer
                        .as_ref()
                        .is_some_and(|footer| footer.text == marker)
                })
        }) {
            message.id
        } else {
            self.send_review(ctx, &fresh, &self.voters(fresh.id).await?)
                .await?
                .id
        };
        sqlx::query("UPDATE event_submissions SET review_message_id = ? WHERE id = ? AND status = 'pending' AND review_message_id IS NULL")
            .bind(message_id.to_string())
            .bind(fresh.id)
            .execute(&self.databases.link)
            .await?;
        Ok(())
    }

    async fn update_review(
        &self,
        ctx: &serenity::Context,
        message_id: serenity::MessageId,
        submission: &Submission,
        voters: &[serenity::UserId],
        resolved: bool,
    ) -> Result<()> {
        let fresh = self
            .submission(submission.id)
            .await?
            .unwrap_or_else(|| submission.clone());
        let resolved = resolved || fresh.status != "pending";
        self.channels
            .review
            .edit_message(
                ctx,
                message_id,
                serenity::EditMessage::new()
                    .embed(review_embed(&fresh, voters, resolved))
                    .components(review_buttons(submission.id, resolved)),
            )
            .await?;
        Ok(())
    }

    async fn dm_resolution(
        &self,
        ctx: &serenity::Context,
        submission: &Submission,
        reason: &str,
    ) -> bool {
        let Some(user_id) = submission
            .submitter_discord_id
            .parse::<u64>()
            .ok()
            .map(serenity::UserId::new)
        else {
            return false;
        };
        let result = async {
            let channel = user_id.create_dm_channel(ctx).await?;
            channel
                .send_message(
                    ctx,
                    serenity::CreateMessage::new()
                        .content(resolution_message(submission, reason))
                        .embed(form_embed(submission, true))
                        .allowed_mentions(serenity::CreateAllowedMentions::new()),
                )
                .await
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, event_id = submission.id, "failed to DM event submitter");
            self.log(
                ctx,
                "Event DM failed",
                format!(
                    "EVT-{}\nSubmitter: <@{}>",
                    submission.id, submission.submitter_discord_id
                ),
                0x00ED_4245,
            )
            .await;
            return false;
        }
        true
    }

    async fn log(&self, ctx: &serenity::Context, title: &str, description: String, colour: u32) {
        if let Err(error) = self
            .channels
            .logs
            .send_message(
                ctx,
                serenity::CreateMessage::new()
                    .embed(
                        serenity::CreateEmbed::new()
                            .title(title)
                            .description(description)
                            .colour(colour),
                    )
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
        {
            tracing::error!(%error, "failed to send event audit log");
        }
    }

    async fn log_test_bypass(
        &self,
        ctx: &serenity::Context,
        event_id: u64,
        user_id: serenity::UserId,
    ) {
        self.log(
            ctx,
            "Event test bypass used",
            format!(
                "EVT-{event_id}\nSubmitter: <@{user_id}>\nBypass: {REQUIRED_PLAYTIME_HOURS}-hour playtime requirement"
            ),
            0x00FF_F11A,
        )
        .await;
    }

    async fn has_pending(&self, user_id: serenity::UserId) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM event_submissions WHERE submitter_discord_id = ? AND event_at > UNIX_TIMESTAMP() AND status IN ('pending', 'approved', 'posting') AND (status = 'pending' OR event_message_id IS NULL)",
        )
        .bind(user_id.to_string())
        .fetch_one(&self.databases.link)
        .await?;
        Ok(count > 0)
    }

    async fn playtime_ticks_60_days(&self, uuid: &str) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT CAST(COALESCE(SUM(value), 0) AS SIGNED) FROM player_stats_per_day WHERE uuid = ? AND type = 'play_time' AND day BETWEEN DATE_SUB(UTC_DATE(), INTERVAL 59 DAY) AND UTC_DATE()",
        )
        .bind(uuid)
        .fetch_one(&self.databases.stats)
        .await
        .context("failed to calculate 60-day playtime")
    }

    async fn submission_denial(
        &self,
        user_id: serenity::UserId,
        linked_name: &str,
        submitted_name: &str,
        uuid: &str,
    ) -> Result<(Option<(&'static str, &'static str)>, bool)> {
        if !minecraft_names_match(linked_name, submitted_name) {
            return Ok((
                Some(("Minecraft account mismatch", "linked_account_mismatch")),
                false,
            ));
        }
        if playtime_check_bypassed(self.test_user_id, user_id) {
            return Ok((None, true));
        }
        let denial = (!meets_playtime_requirement(self.playtime_ticks_60_days(uuid).await?))
            .then_some((
                "At least 50 hours of playtime in the past 60 days is required",
                "insufficient_playtime",
            ));
        Ok((denial, false))
    }

    async fn submission(&self, id: u64) -> Result<Option<Submission>> {
        sqlx::query_as::<_, Submission>(
            "SELECT id, submitter_discord_id, minecraft_username, event_name, explanation, discord_invite, promotion_url, event_at, event_time_input, join_instructions, status, denial_reason, review_message_id, event_message_id FROM event_submissions WHERE id = ? LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.databases.link)
        .await
        .context("failed to load event submission")
    }

    async fn voters(&self, id: u64) -> Result<Vec<serenity::UserId>> {
        let values = sqlx::query_scalar::<_, String>(
            "SELECT voter_discord_id FROM event_votes WHERE event_id = ? ORDER BY created_at, voter_discord_id",
        )
        .bind(id)
        .fetch_all(&self.databases.link)
        .await?;
        Ok(values
            .into_iter()
            .filter_map(|value| value.parse().ok().map(serenity::UserId::new))
            .collect())
    }

    async fn cleanup_drafts(&self) {
        let now = std::time::Instant::now();
        self.drafts
            .lock()
            .await
            .retain(|_, draft| draft.expires_at > now);
    }
}

async fn locked_submission(tx: &mut Transaction<'_, MySql>, id: u64) -> Result<Option<Submission>> {
    sqlx::query_as::<_, Submission>(
        "SELECT id, submitter_discord_id, minecraft_username, event_name, explanation, discord_invite, promotion_url, event_at, event_time_input, join_instructions, status, denial_reason, review_message_id, event_message_id FROM event_submissions WHERE id = ? FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .context("failed to lock event submission")
}

fn first_modal() -> serenity::CreateModal {
    serenity::CreateModal::new("events:form1", "Submit a 6b6t Event — Part 1").components(vec![
        input("Event name", "event_name", 1, 100, false),
        input("Event description", "explanation", 1, 1_000, true),
        input(
            "Linked Minecraft username",
            "minecraft_username",
            1,
            16,
            false,
        ),
        input("Discord invite link", "discord_invite", 1, 512, false),
        input(
            "YouTube video or r/6b6t post",
            "promotion_url",
            1,
            512,
            false,
        ),
    ])
}

fn continue_button(id: Uuid) -> Vec<serenity::CreateActionRow> {
    vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("events:continue:{id}"))
            .label("Continue application")
            .style(serenity::ButtonStyle::Primary),
    ])]
}

fn resolution_message(submission: &Submission, reason: &str) -> String {
    match submission.status.as_str() {
        "approved" => format!(
            "Your event application EVT-{} has been approved and posted in the events channel.",
            submission.id
        ),
        "expired" => format!(
            "Your event application EVT-{} has expired because its start time has passed. Submit a new application with a future date if you would like to reschedule.",
            submission.id
        ),
        _ => format!(
            "Your event application EVT-{} was not approved.\nReason: {reason}",
            submission.id
        ),
    }
}

fn valid_utc_offset(value: &str) -> bool {
    matches!(value.as_bytes(), [b'+' | b'-', a, b, b':', c, d]
        if [a, b, c, d].iter().all(|byte| byte.is_ascii_digit()))
}

fn second_modal(id: Uuid) -> serenity::CreateModal {
    serenity::CreateModal::new(format!("events:form2:{id}"), "Submit a 6b6t Event — Part 2")
        .components(vec![
            serenity::CreateActionRow::InputText(
                serenity::CreateInputText::new(
                    serenity::InputTextStyle::Short,
                    "Date and time",
                    "event_time",
                )
                .placeholder("2026-09-15 20:00 UTC+02:00")
                .min_length(22)
                .max_length(64)
                .required(true),
            ),
            input("How to join", "join_instructions", 1, 1_000, true),
        ])
}

fn input(label: &str, id: &str, min: u16, max: u16, paragraph: bool) -> serenity::CreateActionRow {
    serenity::CreateActionRow::InputText(
        serenity::CreateInputText::new(
            if paragraph {
                serenity::InputTextStyle::Paragraph
            } else {
                serenity::InputTextStyle::Short
            },
            label,
            id,
        )
        .min_length(min)
        .max_length(max)
        .required(true),
    )
}

fn menu_embed() -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
                .title("6b6t Events")
                .description(format!("Discover events organized by the 6b6t community.\n\nTo apply, link your Minecraft account and play at least {REQUIRED_PLAYTIME_HOURS} hours in the past 60 days. Playtime is measured by UTC calendar days. Provide an event description, your linked Minecraft username, a Discord invite, a YouTube video or r/6b6t post, a future start time with a UTC offset, and joining instructions.\n\nThree staff approvals are required. Approved events appear here immediately and are published to following servers after two hours, provided the event has not started.\n\n{DISCLAIMER}\n\nSelect Apply to begin."))
                .thumbnail("https://www.6b6t.org/logo.png")
                .colour(0x00FF_F11A)
}

fn menu_message() -> serenity::CreateMessage {
    serenity::CreateMessage::new()
        .embed(menu_embed())
        .components(vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(APPLY_ID)
                .label("Apply")
                .style(serenity::ButtonStyle::Primary),
        ])])
        .allowed_mentions(serenity::CreateAllowedMentions::new())
}

fn approved_message(submission: &Submission) -> serenity::CreateMessage {
    serenity::CreateMessage::new()
        .nonce(serenity::Nonce::String(format!("e{}", submission.id)))
        .enforce_nonce(true)
        .content(format!("<@&{}>", config::EVENTS_ROLE_ID))
        .embed(
            form_embed(submission, true)
                .field(
                    "Organizer",
                    format!("<@{}>", submission.submitter_discord_id),
                    false,
                )
                .field("Notice", DISCLAIMER, false)
                .footer(serenity::CreateEmbedFooter::new(format!(
                    "EVT-{}",
                    submission.id
                )))
                .colour(0x00FF_F11A),
        )
        .allowed_mentions(serenity::CreateAllowedMentions::new().roles([config::EVENTS_ROLE_ID]))
}

fn form_embed(submission: &Submission, include_private_name: bool) -> serenity::CreateEmbed {
    let mut embed = serenity::CreateEmbed::new()
        .title(&submission.event_name)
        .description(&submission.explanation)
        .field(
            "Discord Server",
            format!("<{}>", submission.discord_invite),
            false,
        )
        .field("Event Post", &submission.promotion_url, false)
        .field(
            "Date",
            format!(
                "{}\n<t:{}:F> • <t:{}:R>",
                submission.event_time_input, submission.event_at, submission.event_at
            ),
            false,
        )
        .field("How to join", &submission.join_instructions, false)
        .thumbnail("https://www.6b6t.org/logo.png");
    if include_private_name {
        embed = embed.field("Minecraft Username", &submission.minecraft_username, false);
    }
    embed
}

fn review_embed(
    submission: &Submission,
    voters: &[serenity::UserId],
    resolved: bool,
) -> serenity::CreateEmbed {
    let voter_text = if voters.is_empty() {
        "None".into()
    } else {
        voters
            .iter()
            .map(|id| format!("<@{id}>"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let remaining = 3usize.saturating_sub(voters.len());
    let status = match submission.status.as_str() {
        "approved" | "posting" => "Approved".into(),
        "denied" => format!(
            "Denied: {}",
            submission
                .denial_reason
                .as_deref()
                .unwrap_or("No reason recorded")
        ),
        "expired" => "Expired — the event start time has passed".into(),
        _ => format!("Pending — {remaining} approval(s) remaining"),
    };
    form_embed(submission, true)
        .title(format!("EVT-{} — {}", submission.id, submission.event_name))
        .field(
            "Submitter",
            format!("<@{}>", submission.submitter_discord_id),
            false,
        )
        .field("Approvals", voter_text, false)
        .field("Status", status, false)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "EVT-{}",
            submission.id
        )))
        .colour(match submission.status.as_str() {
            "denied" | "auto_denied" => 0x00ED_4245,
            "approved" | "posting" => 0x0057_F287,
            _ if resolved => 0x0099_AAB5,
            _ => 0x00FF_F11A,
        })
}

fn review_buttons(id: u64, disabled: bool) -> Vec<serenity::CreateActionRow> {
    vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("events:approve:{id}"))
            .label("Approve")
            .style(serenity::ButtonStyle::Success)
            .disabled(disabled),
        serenity::CreateButton::new(format!("events:deny:{id}"))
            .label("Deny")
            .style(serenity::ButtonStyle::Danger)
            .disabled(disabled),
    ])]
}

fn is_menu_message(ctx: &serenity::Context, message: &serenity::Message) -> bool {
    message.author.id == ctx.cache.current_user().id
        && message.components.iter().flat_map(|row| &row.components).any(|component| {
            matches!(component, serenity::ActionRowComponent::Button(button) if matches!(&button.data, serenity::ButtonKind::NonLink { custom_id, .. } if custom_id == APPLY_ID))
        })
}

fn is_reviewer(member: &serenity::Member) -> bool {
    member.roles.contains(&config::TERMINATOR_ROLE_ID)
        || member.roles.contains(&config::MARKETER_ROLE_ID)
}

fn modal_fields(interaction: &serenity::ModalInteraction) -> HashMap<String, String> {
    interaction
        .data
        .components
        .iter()
        .flat_map(|row| &row.components)
        .filter_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) => input
                .value
                .as_ref()
                .map(|value| (input.custom_id.clone(), value.trim().to_owned())),
            _ => None,
        })
        .collect()
}

fn required_field(fields: &HashMap<String, String>, id: &str, max: usize) -> Result<String> {
    let label = id.replace('_', " ");
    let value = fields
        .get(id)
        .with_context(|| format!("please provide {label}"))?
        .trim();
    if value.is_empty() || value.chars().count() > max {
        bail!("{label} must contain between 1 and {max} characters");
    }
    Ok(value.to_owned())
}

fn minecraft_name(value: &str) -> Result<String> {
    if !(1..=16).contains(&value.len())
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("Minecraft username must contain only 1-16 letters, numbers, or underscores");
    }
    Ok(value.to_owned())
}

fn validate_invite(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value).context("Discord invite must be a valid URL")?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let code = if host == "discord.gg" {
        Some(url.path().trim_matches('/'))
    } else if matches!(host.as_str(), "discord.com" | "www.discord.com") {
        url.path()
            .strip_prefix("/invite/")
            .map(|path| path.trim_end_matches('/'))
    } else {
        None
    };
    let valid = code.is_some_and(|code| {
        !code.is_empty()
            && code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    });
    if url.scheme() != "https" || !valid {
        bail!(
            "Discord invite must be an https://discord.gg/... or https://discord.com/invite/... URL"
        );
    }
    Ok(url.to_string())
}

fn validate_promotion(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value).context("event post must be a valid URL")?;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    let segments: Vec<_> = url.path().trim_matches('/').split('/').collect();
    let video_id = |id: &str| {
        id.len() == 11
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    };
    let youtube = match host.as_str() {
        "youtu.be" => segments.len() == 1 && video_id(segments[0]),
        "youtube.com" => {
            (url.path() == "/watch"
                && url
                    .query_pairs()
                    .any(|(key, value)| key == "v" && video_id(&value)))
                || (segments.len() == 2
                    && matches!(segments[0], "shorts" | "live" | "embed")
                    && video_id(segments[1]))
        }
        _ => false,
    };
    let reddit = matches!(host.as_str(), "reddit.com" | "old.reddit.com")
        && segments.len() >= 4
        && segments[0] == "r"
        && segments[1].eq_ignore_ascii_case("6b6t")
        && segments[2] == "comments"
        && !segments[3].is_empty()
        && segments[3].bytes().all(|byte| byte.is_ascii_alphanumeric());
    if url.scheme() != "https" || !(youtube || reddit) {
        bail!("event post must be an HTTPS YouTube URL or a post in reddit.com/r/6b6t");
    }
    Ok(url.to_string())
}

fn parse_event_time(value: &str) -> Result<i64> {
    let (date, offset) = value
        .split_once(" UTC")
        .context("date must use YYYY-MM-DD HH:MM UTC±HH:MM")?;
    let local = NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M")
        .context("date must use YYYY-MM-DD HH:MM")?;
    if !valid_utc_offset(offset) {
        bail!("UTC offset must use +HH:MM or -HH:MM");
    }
    let hours: i32 = offset[1..3].parse().context("invalid UTC offset hour")?;
    let minutes: i32 = offset[4..6].parse().context("invalid UTC offset minute")?;
    if hours > 14 || minutes > 59 || (hours == 14 && minutes != 0) {
        bail!("UTC offset is outside the valid -14:00 to +14:00 range");
    }
    let seconds = (hours * 60 + minutes) * 60;
    let seconds = if offset.starts_with('-') {
        -seconds
    } else {
        seconds
    };
    let zone = FixedOffset::east_opt(seconds).context("invalid UTC offset")?;
    let timestamp = zone
        .from_local_datetime(&local)
        .single()
        .context("event time is ambiguous")?
        .timestamp();
    if timestamp <= Utc::now().timestamp() {
        bail!("event time must be in the future");
    }
    Ok(timestamp)
}

fn parse_event_id(value: &str) -> Result<u64> {
    let id = value.parse::<u64>().context("invalid event ID")?;
    if id == 0 {
        bail!("invalid event ID");
    }
    Ok(id)
}

fn parse_message_id(value: &str) -> Option<serenity::MessageId> {
    value.parse::<u64>().ok().map(serenity::MessageId::new)
}

fn meets_playtime_requirement(playtime_ticks: i64) -> bool {
    playtime_ticks >= REQUIRED_PLAYTIME_TICKS
}

fn minecraft_names_match(linked_name: &str, submitted_name: &str) -> bool {
    linked_name.eq_ignore_ascii_case(submitted_name)
}

fn playtime_check_bypassed(
    configured_user: Option<serenity::UserId>,
    submitting_user: serenity::UserId,
) -> bool {
    configured_user == Some(submitting_user)
}

fn is_unknown_message(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response))
            if response.error.code == 10_008
    )
}

async fn component_reply(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    content: impl Into<String>,
) -> Result<()> {
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

async fn modal_reply(
    ctx: &serenity::Context,
    interaction: &serenity::ModalInteraction,
    content: impl Into<String>,
    components: Vec<serenity::CreateActionRow>,
) -> Result<()> {
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike as _;

    #[test]
    fn parses_explicit_utc_offsets() {
        let future_year = Utc::now().year() + 2;
        let value = format!("{future_year}-08-22 20:00 UTC+02:30");
        let expected =
            chrono::DateTime::parse_from_rfc3339(&format!("{future_year}-08-22T20:00:00+02:30"))
                .unwrap()
                .timestamp();
        assert_eq!(parse_event_time(&value).unwrap(), expected);
        assert!(parse_event_time("2026-08-22 20:00 Europe/Berlin").is_err());
    }

    #[test]
    fn validates_supported_urls() {
        assert!(validate_invite("https://discord.gg/6b6t").is_ok());
        assert!(validate_invite("https://example.com/6b6t").is_err());
        assert!(validate_promotion("https://youtu.be/dQw4w9WgXcQ").is_ok());
        assert!(validate_promotion("https://reddit.com/r/6b6t/comments/example").is_ok());
        assert!(validate_promotion("https://reddit.com/r/other/comments/example").is_err());
    }

    #[test]
    fn playtime_threshold_is_exact() {
        assert_eq!(REQUIRED_PLAYTIME_TICKS, 3_600_000);
        assert!(!meets_playtime_requirement(3_599_999));
        assert!(meets_playtime_requirement(3_600_000));
        assert!(meets_playtime_requirement(3_600_001));
        assert!(meets_playtime_requirement(10 * 24 * 60 * 60 * 20)); // Ten days in ticks.
    }

    #[test]
    fn minecraft_names_are_compared_case_insensitively() {
        assert!(minecraft_names_match("adzfoofie", "AdzFoofie"));
        assert!(!minecraft_names_match("adzfoofie", "other_player"));
    }

    #[test]
    fn playtime_bypass_only_matches_the_configured_user() {
        let configured = serenity::UserId::new(681_552_494_064_697_350);
        assert!(playtime_check_bypassed(Some(configured), configured));
        assert!(!playtime_check_bypassed(
            Some(configured),
            serenity::UserId::new(1)
        ));
        assert!(!playtime_check_bypassed(None, configured));
    }

    #[test]
    fn field_limits_are_enforced_server_side() {
        let fields = HashMap::from([("explanation".into(), "a".repeat(1_001))]);
        assert!(required_field(&fields, "explanation", 1_000).is_err());
        let fields = HashMap::from([("explanation".into(), "valid".into())]);
        assert_eq!(
            required_field(&fields, "explanation", 1_000).unwrap(),
            "valid"
        );
    }

    #[test]
    fn button_ids_only_accept_positive_event_ids() {
        assert_eq!(parse_event_id("21").unwrap(), 21);
        assert_eq!(parse_event_id(&u64::MAX.to_string()).unwrap(), u64::MAX);
        assert!(parse_event_id("0").is_err());
        assert!(parse_event_id("EVT-21").is_err());
    }

    #[test]
    fn malformed_times_and_non_post_urls_are_rejected() {
        for offset in ["+0é00", "+-1:00", "+00:-1", "+14:01", "+00:60", "+0:00"] {
            assert!(parse_event_time(&format!("2099-09-15 20:00 UTC{offset}")).is_err());
        }
        for url in [
            "https://discord.gg/",
            "https://discord.com/invite/",
            "https://discord.gg/a/b",
        ] {
            assert!(validate_invite(url).is_err());
        }
        for url in [
            "https://youtube.com/",
            "https://youtube.com/watch",
            "https://youtu.be/",
            "https://reddit.com/r/6b6t/",
            "https://reddit.com/r/6b6t/comments/",
        ] {
            assert!(validate_promotion(url).is_err());
        }
        assert!(validate_promotion("https://www.youtube.com/watch?v=dQw4w9WgXcQ").is_ok());
        assert!(validate_promotion("https://youtube.com/shorts/dQw4w9WgXcQ").is_ok());
    }

    #[test]
    fn event_messages_show_requirements_and_correct_resolution() {
        let mut submission = Submission {
            id: u64::MAX,
            submitter_discord_id: "1".into(),
            minecraft_username: "Player".into(),
            event_name: "Community meetup".into(),
            explanation: "Meet other players.".into(),
            discord_invite: "https://discord.gg/6b6t".into(),
            promotion_url: "https://youtu.be/dQw4w9WgXcQ".into(),
            event_at: 4_102_444_800,
            event_time_input: "2100-01-01 00:00 UTC+00:00".into(),
            join_instructions: "Join the server.".into(),
            status: "approved".into(),
            denial_reason: None,
            review_message_id: None,
            event_message_id: Some("2".into()),
        };
        assert!(resolution_message(&submission, "").contains("approved and posted"));
        submission.status = "denied".into();
        assert!(resolution_message(&submission, "Missing details").contains("Missing details"));
        submission.status = "expired".into();
        assert!(resolution_message(&submission, "").contains("start time has passed"));
        let menu = serde_json::to_value(menu_message()).unwrap();
        let text = menu["embeds"][0]["description"].as_str().unwrap();
        assert!(text.contains("50 hours"));
        assert!(text.contains("60 days"));
        assert!(!text.contains("100 hours"));
        let post = serde_json::to_value(approved_message(&submission)).unwrap();
        assert!(post["nonce"].as_str().unwrap().len() <= 25);
        assert_eq!(post["enforce_nonce"], true);
        let review = serde_json::to_value(review_embed(&submission, &[], true)).unwrap();
        assert!(review.to_string().contains("Expired"));
    }
}
