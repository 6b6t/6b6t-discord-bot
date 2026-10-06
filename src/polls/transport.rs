//! The two Discord calls the poll worker makes, behind a trait so the
//! refresh/close/finalize schedules can be tested with a fake transport.

use anyhow::Result;
use futures::future::BoxFuture;
use poise::serenity_prelude as serenity;

use crate::config as bot_config;

/// What happened to an edit.
#[derive(Debug, Eq, PartialEq)]
pub enum EditResult {
    Done,
    /// Discord says the message no longer exists.
    Gone,
}

/// One message edit. `closed` and `counts` describe what the edit shows, so a
/// fake transport can tell an open redraw from the final message without
/// reading the embed; the real transport only sends `edit`.
pub struct EditRequest {
    pub channel_id: u64,
    pub message_id: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub closed: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    pub counts: Option<Vec<u32>>,
    pub edit: serenity::EditMessage,
}

pub trait PollTransport: Send + Sync {
    /// Replaces the poll message's embed and buttons.
    fn edit_message(&self, request: EditRequest) -> BoxFuture<'_, Result<EditResult>>;

    /// True only when Discord confirms the user is no longer in the server. Any
    /// other outcome (still there, or the check failed) keeps the vote.
    fn member_gone(&self, user_id: u64) -> BoxFuture<'_, bool>;
}

/// The real thing, over serenity's HTTP client.
pub struct DiscordTransport<'a> {
    ctx: &'a serenity::Context,
}

impl<'a> DiscordTransport<'a> {
    pub fn new(ctx: &'a serenity::Context) -> Self {
        Self { ctx }
    }
}

impl PollTransport for DiscordTransport<'_> {
    fn edit_message(&self, request: EditRequest) -> BoxFuture<'_, Result<EditResult>> {
        Box::pin(async move {
            let edit = serenity::ChannelId::new(request.channel_id)
                .edit_message(
                    self.ctx,
                    serenity::MessageId::new(request.message_id),
                    request.edit,
                )
                .await;
            match edit {
                Ok(_) => Ok(EditResult::Done),
                Err(error) if is_unknown_message(&error) => Ok(EditResult::Gone),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn member_gone(&self, user_id: u64) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            match bot_config::GUILD_ID
                .member(self.ctx, serenity::UserId::new(user_id))
                .await
            {
                Ok(_) => false,
                Err(error) if is_unknown_member(&error) => true,
                Err(error) => {
                    tracing::warn!(%error, "could not check a voter's membership; keeping the vote");
                    false
                }
            }
        })
    }
}

fn is_unknown_message(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response))
            if response.error.code == 10_008
    )
}

fn is_unknown_member(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response))
            if response.error.code == 10_007 || response.error.code == 10_013
    )
}
