use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    command_moderation, config,
    moderation::{self, ApprovalAction},
    state::AppState,
};

pub async fn handle(
    ctx: &serenity::Context,
    data: &AppState,
    interaction: &serenity::Interaction,
) -> Result<()> {
    match interaction {
        serenity::Interaction::Component(component)
            if component.data.custom_id == "legend_role_menu" =>
        {
            legend_role(ctx, component).await
        }
        serenity::Interaction::Component(component)
            if parse_horizon_side(&component.data.custom_id).is_some() =>
        {
            horizon_role(ctx, component).await
        }
        serenity::Interaction::Component(component)
            if component.data.custom_id.starts_with("motd:") =>
        {
            motd_button(ctx, data, component).await
        }
        serenity::Interaction::Component(component) => approval(ctx, data, component).await,
        serenity::Interaction::Modal(modal) if modal.data.custom_id.starts_with("motd:") => {
            motd_modal(ctx, data, modal).await
        }
        _ => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HorizonSide {
    Hunt,
    Protect,
}

fn parse_horizon_side(custom_id: &str) -> Option<HorizonSide> {
    match custom_id {
        config::HUNT_HORIZON_BUTTON_ID => Some(HorizonSide::Hunt),
        config::PROTECT_HORIZON_BUTTON_ID => Some(HorizonSide::Protect),
        _ => None,
    }
}

async fn horizon_role(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
) -> Result<()> {
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Defer(
                serenity::CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;
    let result = update_horizon_role(ctx, interaction).await;
    let content = match &result {
        Ok(content) => content.clone(),
        Err(error) => {
            tracing::error!(%error, user_id = %interaction.user.id, "failed to update Horizon role");
            format!("I couldn't update your Horizon role: {error}")
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

async fn update_horizon_role(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
) -> Result<String> {
    static ROLE_UPDATE_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
        std::sync::OnceLock::new();
    let _guard = ROLE_UPDATE_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    if interaction.channel_id != config::HORIZON_ROLE_MENU_ID {
        anyhow::bail!("this button is not in the self-role channel");
    }
    let side =
        parse_horizon_side(&interaction.data.custom_id).context("unknown Horizon role button")?;
    let guild_id = interaction.guild_id.context("missing guild")?;
    let (selected, opposing) = match side {
        HorizonSide::Hunt => (
            config::HUNT_HORIZON_ROLE_ID,
            config::PROTECT_HORIZON_ROLE_ID,
        ),
        HorizonSide::Protect => (
            config::PROTECT_HORIZON_ROLE_ID,
            config::HUNT_HORIZON_ROLE_ID,
        ),
    };
    let member = guild_id.member(ctx, interaction.user.id).await?;
    let has_selected = member.roles.contains(&selected);
    let has_opposing = member.roles.contains(&opposing);
    if has_selected && !has_opposing {
        member.remove_role(ctx, selected).await?;
        return Ok(format!("Removed <@&{selected}>."));
    }
    if has_opposing {
        member.remove_role(ctx, opposing).await?;
    }
    if !has_selected {
        member.add_role(ctx, selected).await?;
    }
    Ok(format!("You are now on <@&{selected}>."))
}

async fn legend_role(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
) -> Result<()> {
    let member = interaction
        .member
        .as_ref()
        .context("role menu used outside a guild")?;
    if !member.roles.contains(&config::ROLE_MENU_REQUIRED_ROLE_ID)
        && !moderation::is_administrator(member)
    {
        reply_ephemeral(
            ctx,
            interaction,
            format!(
                "You don't have the <@&{}> role.",
                config::ROLE_MENU_REQUIRED_ROLE_ID
            ),
        )
        .await?;
        return Ok(());
    }
    let serenity::ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind
    else {
        return Ok(());
    };
    let selected = values.first().context("role menu selection was empty")?;
    let guild_id = interaction.guild_id.context("missing guild")?;
    let member = guild_id.member(ctx, interaction.user.id).await?;
    let had_color_role = member
        .roles
        .iter()
        .any(|role| config::ROLE_MENU_ROLE_IDS.contains(role));
    for role in config::ROLE_MENU_ROLE_IDS {
        if member.roles.contains(role) {
            member.remove_role(ctx, *role).await?;
        }
    }
    if matches!(selected.as_str(), "clear_top" | "clear_bottom") {
        reply_ephemeral(
            ctx,
            interaction,
            if had_color_role {
                "Your color role has been removed."
            } else {
                "You do not currently have a color role."
            },
        )
        .await?;
    } else {
        let role_id = selected.parse::<u64>().context("invalid role menu value")?;
        let role_id = serenity::RoleId::new(role_id);
        if !config::ROLE_MENU_ROLE_IDS.contains(&role_id) {
            anyhow::bail!("role menu selected an unconfigured role")
        }
        member.add_role(ctx, role_id).await?;
        reply_ephemeral(
            ctx,
            interaction,
            format!("You have been given the color: <@&{role_id}>"),
        )
        .await?;
    }
    Ok(())
}

async fn approval(
    ctx: &serenity::Context,
    data: &AppState,
    interaction: &serenity::ComponentInteraction,
) -> Result<()> {
    let Some((prefix, approve, id)) = parse_approval_id(&interaction.data.custom_id) else {
        return Ok(());
    };
    let Some(request) = data.pending.get(id).await else {
        reply_ephemeral(
            ctx,
            interaction,
            "This request has expired or has already been processed.",
        )
        .await?;
        return Ok(());
    };
    let member = interaction
        .member
        .as_ref()
        .context("approval used outside a guild")?;
    if !moderation::has_role(member, config::TERMINATOR_ROLE_ID) {
        reply_ephemeral(
            ctx,
            interaction,
            "Only members with the Terminator role can process this request.",
        )
        .await?;
        return Ok(());
    }
    if approve && interaction.user.id == request.submitter_id {
        reply_ephemeral(
            ctx,
            interaction,
            "You cannot approve your own request. A different Terminator must approve it.",
        )
        .await?;
        return Ok(());
    }
    let Some(request) = claim_approval(ctx, data, interaction, id).await? else {
        return Ok(());
    };
    if !approve {
        let embed = resolved_approval_embed(interaction, "Rejected", 0x00ED_4245);
        let mut response = serenity::CreateInteractionResponseMessage::new()
            .content(format!("Request rejected by <@{}>.", interaction.user.id))
            .components(vec![moderation::approval_buttons(prefix, id, true)]);
        if let Some(embed) = embed {
            response = response.embed(embed);
        }
        if let Err(error) = interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(response),
            )
            .await
        {
            data.pending.restore(request).await;
            return Err(error.into());
        }
        return Ok(());
    }
    if let Err(error) = interaction
        .create_response(ctx, serenity::CreateInteractionResponse::Acknowledge)
        .await
    {
        data.pending.restore(request).await;
        return Err(error.into());
    }
    let action_result = execute_approval_action(ctx, data, &request, interaction).await;
    if let Err(error) = action_result {
        tracing::error!(%error, request_id = %id, approver_id = %interaction.user.id, "approval action failed");
        data.pending.restore(request).await;
        let embed =
            resolved_approval_embed(interaction, "Action failed; retry available", 0x00ED_4245);
        let mut edit = serenity::EditInteractionResponse::new()
            .content("The approval action failed. The request is still available to retry.")
            .components(vec![moderation::approval_buttons(prefix, id, false)]);
        if let Some(embed) = embed {
            edit = edit.embed(embed);
        }
        interaction
            .create_followup(
                ctx,
                serenity::CreateInteractionResponseFollowup::new()
                    .content(format!("The approval action failed: {error}"))
                    .ephemeral(true),
            )
            .await?;
        interaction.edit_response(ctx, edit).await?;
        return Ok(());
    }
    let embed = resolved_approval_embed(interaction, "Approved", 0x0057_F287);
    let mut edit = serenity::EditInteractionResponse::new()
        .content(format!("Request approved by <@{}>.", interaction.user.id))
        .components(vec![moderation::approval_buttons(prefix, id, true)]);
    if let Some(embed) = embed {
        edit = edit.embed(embed);
    }
    interaction.edit_response(ctx, edit).await?;
    log_approval(ctx, data, &request, interaction).await;
    Ok(())
}

async fn claim_approval(
    ctx: &serenity::Context,
    data: &AppState,
    interaction: &serenity::ComponentInteraction,
    id: uuid::Uuid,
) -> Result<Option<crate::moderation::ApprovalRequest>> {
    // Claim before acting because moderators can click concurrently.
    let request = data.pending.remove(id).await;
    if request.is_none() {
        reply_ephemeral(
            ctx,
            interaction,
            "This request has already been processed by another moderator.",
        )
        .await?;
    }
    Ok(request)
}

async fn execute_approval_action(
    ctx: &serenity::Context,
    data: &AppState,
    request: &crate::moderation::ApprovalRequest,
    interaction: &serenity::ComponentInteraction,
) -> Result<()> {
    match &request.action {
        ApprovalAction::GuildImage {
            image_url,
            location,
        } => {
            command_moderation::set_guild_image(
                &ctx.http,
                &data.http,
                request.guild_id,
                *location,
                image_url,
            )
            .await?;
        }
        ApprovalAction::Ban {
            target_id,
            reason,
            delete_message_days,
        } => {
            request
                .guild_id
                .ban_with_reason(
                    &ctx.http,
                    *target_id,
                    *delete_message_days,
                    format!("Approved by {}: {reason}", interaction.user.tag()),
                )
                .await?;
        }
        ApprovalAction::Unban { target_id, .. } => {
            request
                .guild_id
                .unban(&ctx.http, *target_id)
                .await
                .context("failed to unban user")?;
        }
        ApprovalAction::MediaFrequency { requested, .. } => {
            data.media.set_frequency(*requested).await?;
        }
        ApprovalAction::MiniTerminator { target_id, add } => {
            command_moderation::apply_mini_role(&ctx.http, request.guild_id, *target_id, *add)
                .await?;
        }
    }
    Ok(())
}

fn resolved_approval_embed(
    interaction: &serenity::ComponentInteraction,
    status: &str,
    colour: u32,
) -> Option<serenity::CreateEmbed> {
    let mut embed = interaction.message.embeds.first()?.clone();
    if let Some(field) = embed.fields.iter_mut().find(|field| field.name == "Status") {
        status.clone_into(&mut field.value);
    } else {
        embed
            .fields
            .push(serenity::EmbedField::new("Status", status, true));
    }
    Some(serenity::CreateEmbed::from(embed).colour(colour))
}

fn parse_approval_id(custom_id: &str) -> Option<(&str, bool, Uuid)> {
    for prefix in ["banner", "ban", "unban", "mediafreq", "mini"] {
        for (action, approve) in [("approve", true), ("reject", false)] {
            if let Some(value) = custom_id.strip_prefix(&format!("{prefix}_{action}_")) {
                return Uuid::parse_str(value).ok().map(|id| (prefix, approve, id));
            }
        }
    }
    None
}

async fn motd_button(
    ctx: &serenity::Context,
    data: &AppState,
    interaction: &serenity::ComponentInteraction,
) -> Result<()> {
    let Some((action, request_id)) = parse_motd_id(&interaction.data.custom_id) else {
        return Ok(());
    };
    if action == "revision" {
        let input =
            serenity::CreateInputText::new(serenity::InputTextStyle::Paragraph, "Reason", "reason")
                .min_length(3)
                .max_length(1_000)
                .placeholder("Explain what needs to be changed.")
                .required(true);
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::Modal(
                    serenity::CreateModal::new(
                        format!("motd:revision_modal:{request_id}"),
                        "Request MOTD Revision",
                    )
                    .components(vec![serenity::CreateActionRow::InputText(input)]),
                ),
            )
            .await?;
        return Ok(());
    }
    if action != "approve" {
        return Ok(());
    }
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Defer(
                serenity::CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;
    let result = post_motd(
        data,
        interaction
            .member
            .as_ref()
            .context("MOTD review used outside a guild")?,
        "approve",
        request_id,
        None,
    )
    .await;
    interaction
        .edit_response(
            ctx,
            serenity::EditInteractionResponse::new()
                .content(result.unwrap_or_else(|error| format!("Failed to approve MOTD: {error}"))),
        )
        .await?;
    Ok(())
}

async fn motd_modal(
    ctx: &serenity::Context,
    data: &AppState,
    interaction: &serenity::ModalInteraction,
) -> Result<()> {
    let Some((action, request_id)) = parse_motd_id(&interaction.data.custom_id) else {
        return Ok(());
    };
    if action != "revision_modal" {
        return Ok(());
    }
    let reason = interaction
        .data
        .components
        .iter()
        .flat_map(|row| &row.components)
        .find_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) if input.custom_id == "reason" => {
                input.value.clone()
            }
            _ => None,
        })
        .context("revision modal had no reason")?;
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Defer(
                serenity::CreateInteractionResponseMessage::new().ephemeral(true),
            ),
        )
        .await?;
    let result = post_motd(
        data,
        interaction
            .member
            .as_ref()
            .context("MOTD review used outside a guild")?,
        "revision",
        request_id,
        Some(reason),
    )
    .await;
    interaction
        .edit_response(
            ctx,
            serenity::EditInteractionResponse::new().content(
                result.unwrap_or_else(|error| format!("Failed to request revision: {error}")),
            ),
        )
        .await?;
    Ok(())
}

fn parse_motd_id(value: &str) -> Option<(&str, &str)> {
    let mut parts = value.splitn(3, ':');
    (parts.next()? == "motd").then_some((parts.next()?, parts.next()?))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MotdRequest<'a> {
    action: &'a str,
    request_id: &'a str,
    reason: Option<String>,
    reviewer: MotdReviewer,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MotdReviewer {
    id: String,
    username: String,
    role_ids: Vec<String>,
}
#[derive(serde::Deserialize)]
struct MotdResponse {
    success: bool,
    error: Option<String>,
    data: Option<MotdResponseData>,
}
#[derive(serde::Deserialize)]
struct MotdResponseData {
    message: String,
}

async fn post_motd(
    data: &AppState,
    member: &serenity::Member,
    action: &str,
    request_id: &str,
    reason: Option<String>,
) -> Result<String> {
    let secret = data
        .environment
        .motd_review_secret
        .as_deref()
        .context("MOTD_REVIEW_BOT_SECRET is not configured")?;
    let response = data
        .http
        .post(&data.environment.motd_review_url)
        .bearer_auth(secret)
        .json(&MotdRequest {
            action,
            request_id,
            reason,
            reviewer: MotdReviewer {
                id: member.user.id.to_string(),
                username: member
                    .user
                    .global_name
                    .clone()
                    .unwrap_or_else(|| member.user.tag()),
                role_ids: member.roles.iter().map(ToString::to_string).collect(),
            },
        })
        .send()
        .await?;
    let status = response.status();
    let payload: MotdResponse = response
        .json()
        .await
        .context("website returned an invalid MOTD review response")?;
    if !status.is_success() || !payload.success {
        anyhow::bail!(
            "{}",
            payload
                .error
                .unwrap_or_else(|| format!("website returned HTTP {status}"))
        )
    }
    Ok(payload.data.map_or_else(
        || format!("Processed MOTD request {request_id}."),
        |data| data.message,
    ))
}

async fn reply_ephemeral(
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

async fn log_approval(
    ctx: &serenity::Context,
    data: &AppState,
    request: &crate::moderation::ApprovalRequest,
    interaction: &serenity::ComponentInteraction,
) {
    let Some(channel_id) = data.environment.log_channel_id else {
        return;
    };
    let description = format!(
        "{}\nSubmitted by <@{}> ({})\nApproved by <@{}> ({})",
        approval_details(&request.action),
        request.submitter_id,
        request.submitter_tag,
        interaction.user.id,
        interaction.user.tag()
    );
    if let Err(error) = channel_id
        .send_message(
            ctx,
            serenity::CreateMessage::new().embed(
                serenity::CreateEmbed::new()
                    .title(approval_title(&request.action))
                    .description(description)
                    .colour(0x0057_F287),
            ),
        )
        .await
    {
        tracing::error!(%error, "failed to send approval audit log");
    }
}

fn approval_title(action: &ApprovalAction) -> &'static str {
    match action {
        ApprovalAction::GuildImage { location, .. } => location.change_title(),
        ApprovalAction::Ban { .. } => "User Banned",
        ApprovalAction::Unban { .. } => "User Unbanned",
        ApprovalAction::MediaFrequency { .. } => "Media Frequency Changed",
        ApprovalAction::MiniTerminator { .. } => "Mini-Terminator Role Changed",
    }
}

fn approval_details(action: &ApprovalAction) -> String {
    match action {
        ApprovalAction::GuildImage { image_url, .. } => format!("[View image]({image_url})"),
        ApprovalAction::Ban {
            target_id,
            reason,
            delete_message_days,
        } => format!(
            "Target: <@{target_id}>\nReason: {reason}\nMessages deleted: {delete_message_days} day(s)"
        ),
        ApprovalAction::Unban { target_id, reason } => {
            format!("Target: <@{target_id}>\nReason: {reason}")
        }
        ApprovalAction::MediaFrequency { current, requested } => {
            format!(
                "Previous frequency: every {current} message(s)\nNew frequency: every {requested} message(s)"
            )
        }
        ApprovalAction::MiniTerminator { target_id, add } => format!(
            "Target: <@{target_id}>\nAction: {}",
            if *add { "grant" } else { "remove" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{HorizonSide, parse_approval_id, parse_horizon_side, parse_motd_id};
    use crate::config;
    #[test]
    fn approval_ids_are_strict() {
        let id = uuid::Uuid::new_v4();
        assert_eq!(
            parse_approval_id(&format!("ban_approve_{id}"))
                .map(|(_, approve, parsed)| (approve, parsed)),
            Some((true, id))
        );
        assert!(parse_approval_id("ban_approve_nope").is_none());
    }
    #[test]
    fn motd_ids_preserve_request_value() {
        assert_eq!(
            parse_motd_id("motd:revision:abc:123"),
            Some(("revision", "abc:123"))
        );
    }
    #[test]
    fn horizon_buttons_only_accept_known_sides() {
        assert_eq!(parse_horizon_side("horizon:hunt"), Some(HorizonSide::Hunt));
        assert_eq!(
            parse_horizon_side("horizon:protect"),
            Some(HorizonSide::Protect)
        );
        assert_eq!(parse_horizon_side("horizon:other"), None);
        assert_eq!(
            config::HUNT_HORIZON_ROLE_ID.get(),
            1_540_734_033_959_583_805
        );
        assert_eq!(
            config::PROTECT_HORIZON_ROLE_ID.get(),
            1_540_733_548_133_224_498
        );
        assert_eq!(config::HORIZON_ROLE_MENU_ID, config::UPDATES_ID);
    }
}
