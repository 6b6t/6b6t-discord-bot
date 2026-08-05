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
    for role in config::ROLE_MENU_ROLE_IDS {
        if member.roles.contains(role) {
            member.remove_role(ctx, *role).await?;
        }
    }
    if matches!(selected.as_str(), "clear_top" | "clear_bottom") {
        reply_ephemeral(ctx, interaction, "Your color role has been removed.").await?;
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
    if !approve {
        data.pending.remove(id).await;
        interaction
            .create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content(format!("Request rejected by <@{}>.", interaction.user.id))
                        .components(vec![moderation::approval_buttons(prefix, id, true)]),
                ),
            )
            .await?;
        return Ok(());
    }
    interaction
        .create_response(ctx, serenity::CreateInteractionResponse::Acknowledge)
        .await?;
    match &request.action {
        ApprovalAction::Banner { image_url } => {
            command_moderation::set_banner(&ctx.http, &data.http, request.guild_id, image_url)
                .await?;
        }
        ApprovalAction::Ban {
            target_id,
            reason,
            delete_message_days,
            ..
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
        ApprovalAction::MediaFrequency { requested, .. } => {
            data.media.set_frequency(*requested).await?;
        }
        ApprovalAction::MiniTerminator { target_id, add, .. } => {
            command_moderation::apply_mini_role(&ctx.http, request.guild_id, *target_id, *add)
                .await?;
        }
    }
    data.pending.remove(id).await;
    interaction
        .edit_response(
            ctx,
            serenity::EditInteractionResponse::new()
                .content(format!("Request approved by <@{}>.", interaction.user.id))
                .components(vec![moderation::approval_buttons(prefix, id, true)]),
        )
        .await?;
    log_approval(ctx, data, &request, interaction).await;
    Ok(())
}

fn parse_approval_id(custom_id: &str) -> Option<(&str, bool, Uuid)> {
    for prefix in ["banner", "ban", "mediafreq", "mini"] {
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
        "Submitted by <@{}> ({})\nApproved by <@{}> ({})",
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
                    .title("Moderation Request Approved")
                    .description(description)
                    .colour(0x0057_F287),
            ),
        )
        .await
    {
        tracing::error!(%error, "failed to send approval audit log");
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_approval_id, parse_motd_id};
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
}
