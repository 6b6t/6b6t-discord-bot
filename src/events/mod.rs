mod interactions;
mod messages;
mod ready;
mod role_sync;

use poise::serenity_prelude as serenity;

use crate::state::{AppState, Error};

pub async fn handle(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &AppState,
) -> Result<(), Error> {
    let result = match event {
        serenity::FullEvent::Ready { data_about_bot } => {
            ready::handle(ctx, data, data_about_bot).await
        }
        serenity::FullEvent::Message { new_message } => {
            if let Some(service) = &data.event_submissions {
                service.on_message(ctx, new_message).await;
            }
            messages::create(ctx, data, new_message).await
        }
        serenity::FullEvent::MessageUpdate { event, new, .. } => {
            messages::update_event(ctx, data, event, new.as_ref()).await
        }
        serenity::FullEvent::ReactionAdd { add_reaction } => {
            messages::reaction(ctx, add_reaction, true).await
        }
        serenity::FullEvent::ReactionRemove { removed_reaction } => {
            messages::reaction(ctx, removed_reaction, false).await
        }
        serenity::FullEvent::InteractionCreate { interaction } => {
            interactions::handle(ctx, data, interaction).await
        }
        serenity::FullEvent::MessageDelete {
            channel_id,
            deleted_message_id,
            ..
        } => {
            if let Some(telegram) = &data.telegram {
                telegram
                    .queue_message_delete(*channel_id, *deleted_message_id)
                    .await;
            }
            if let Some(service) = &data.event_submissions {
                service
                    .on_delete(ctx, *channel_id, *deleted_message_id)
                    .await;
            }
            Ok(())
        }
        serenity::FullEvent::MessageDeleteBulk {
            channel_id,
            multiple_deleted_messages_ids,
            ..
        } => {
            if let Some(service) = &data.event_submissions {
                service
                    .on_bulk_delete(ctx, *channel_id, multiple_deleted_messages_ids)
                    .await;
            }
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(error) = &result {
        tracing::error!(event = event.snake_case_name(), %error, "gateway event handler failed");
    }
    result
}
