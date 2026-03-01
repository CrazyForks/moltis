use std::time::Duration;

use {
    async_trait::async_trait,
    secrecy::ExposeSecret,
    slack_morphism::prelude::*,
    tracing::{debug, warn},
};

use moltis_channels::{
    Error as ChannelError, Result as ChannelResult,
    plugin::{ChannelOutbound, ChannelStreamOutbound, StreamEvent, StreamReceiver},
};

use crate::{
    config::StreamMode,
    markdown::{SLACK_MAX_MESSAGE_LEN, chunk_message, markdown_to_slack},
    state::AccountStateMap,
};

/// Minimum chars before the first message is sent during streaming.
const STREAM_MIN_INITIAL_CHARS: usize = 30;

/// Slack outbound message sender.
pub struct SlackOutbound {
    pub(crate) accounts: AccountStateMap,
}

impl SlackOutbound {
    /// Get a Slack client session for the given account.
    fn get_session(
        &self,
        account_id: &str,
    ) -> ChannelResult<(SlackClient<SlackClientHyperHttpsConnector>, SlackApiToken)> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        let state = accounts
            .get(account_id)
            .ok_or_else(|| ChannelError::unknown_account(account_id))?;

        let token_str = state.config.bot_token.expose_secret().clone();
        let token = SlackApiToken::new(SlackApiTokenValue::from(token_str));

        let client = SlackClient::new(
            SlackClientHyperConnector::new()
                .map_err(|e| ChannelError::unavailable(format!("hyper connector: {e}")))?,
        );

        Ok((client, token))
    }

    /// Get the thread_ts for reply threading.
    fn get_thread_ts(&self, account_id: &str, to: &str, reply_to: Option<&str>) -> Option<String> {
        // If we have an explicit reply_to (message_id), use that as thread_ts.
        if let Some(ts) = reply_to {
            return Some(ts.to_string());
        }

        // Check if thread_replies is enabled and we have a stored thread_ts.
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        let state = accounts.get(account_id)?;
        if !state.config.thread_replies {
            return None;
        }
        // Look up by channel_id (any user).
        state
            .pending_threads
            .iter()
            .find(|(k, _)| k.starts_with(&format!("{to}:")))
            .map(|(_, ts)| ts.clone())
    }

    /// Get the edit throttle duration for streaming.
    fn get_edit_throttle(&self, account_id: &str) -> Duration {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_id)
            .map(|s| Duration::from_millis(s.config.edit_throttle_ms))
            .unwrap_or(Duration::from_millis(500))
    }

    /// Get the stream mode for the given account.
    fn get_stream_mode(&self, account_id: &str) -> StreamMode {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_id)
            .map(|s| s.config.stream_mode.clone())
            .unwrap_or_default()
    }
}

/// Post a message to a Slack channel.
async fn post_message(
    client: &SlackClient<SlackClientHyperHttpsConnector>,
    token: &SlackApiToken,
    channel: &str,
    text: &str,
    thread_ts: Option<&str>,
) -> ChannelResult<SlackTs> {
    let session = client.open_session(token);
    let channel_id: SlackChannelId = channel.into();

    let mut req = SlackApiChatPostMessageRequest::new(
        channel_id,
        SlackMessageContent::new().with_text(text.to_string()),
    );

    if let Some(ts) = thread_ts {
        req = req.with_thread_ts(ts.into());
    }

    let resp = session
        .chat_post_message(&req)
        .await
        .map_err(|e| ChannelError::unavailable(format!("chat.postMessage failed: {e}")))?;

    Ok(resp.ts)
}

/// Update an existing message.
async fn update_message(
    client: &SlackClient<SlackClientHyperHttpsConnector>,
    token: &SlackApiToken,
    channel: &str,
    ts: &SlackTs,
    text: &str,
) -> ChannelResult<()> {
    let session = client.open_session(token);
    let channel_id: SlackChannelId = channel.into();

    let req = SlackApiChatUpdateRequest::new(
        channel_id,
        SlackMessageContent::new().with_text(text.to_string()),
        ts.clone(),
    );

    session
        .chat_update(&req)
        .await
        .map_err(|e| ChannelError::unavailable(format!("chat.update failed: {e}")))?;

    Ok(())
}

#[async_trait]
impl ChannelOutbound for SlackOutbound {
    async fn send_text(
        &self,
        account_id: &str,
        to: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> ChannelResult<()> {
        let (client, token) = self.get_session(account_id)?;
        let thread_ts = self.get_thread_ts(account_id, to, reply_to);
        let slack_text = markdown_to_slack(text);

        let chunks = chunk_message(&slack_text, SLACK_MAX_MESSAGE_LEN);
        for chunk in chunks {
            post_message(&client, &token, to, chunk, thread_ts.as_deref()).await?;
        }

        Ok(())
    }

    async fn send_media(
        &self,
        account_id: &str,
        to: &str,
        payload: &moltis_common::types::ReplyPayload,
        reply_to: Option<&str>,
    ) -> ChannelResult<()> {
        // For POC, fall back to text content.
        let text = if payload.text.is_empty() {
            "(media attachment)".to_string()
        } else {
            payload.text.clone()
        };
        self.send_text(account_id, to, &text, reply_to).await
    }

    async fn send_typing(&self, _account_id: &str, _to: &str) -> ChannelResult<()> {
        // Slack bots cannot show typing indicators.
        Ok(())
    }
}

#[async_trait]
impl ChannelStreamOutbound for SlackOutbound {
    async fn send_stream(
        &self,
        account_id: &str,
        to: &str,
        reply_to: Option<&str>,
        mut stream: StreamReceiver,
    ) -> ChannelResult<()> {
        let (client, token) = self.get_session(account_id)?;
        let thread_ts = self.get_thread_ts(account_id, to, reply_to);
        let throttle = self.get_edit_throttle(account_id);

        let mut accumulated = String::new();
        let mut sent_ts: Option<SlackTs> = None;
        let mut last_edit = tokio::time::Instant::now();

        loop {
            match stream.recv().await {
                Some(StreamEvent::Delta(chunk)) => {
                    accumulated.push_str(&chunk);

                    match &sent_ts {
                        None => {
                            // Haven't sent initial message yet.
                            if accumulated.len() >= STREAM_MIN_INITIAL_CHARS {
                                let slack_text = markdown_to_slack(&accumulated);
                                match post_message(
                                    &client,
                                    &token,
                                    to,
                                    &format!("{slack_text}..."),
                                    thread_ts.as_deref(),
                                )
                                .await
                                {
                                    Ok(ts) => {
                                        sent_ts = Some(ts);
                                        last_edit = tokio::time::Instant::now();
                                    },
                                    Err(e) => {
                                        warn!(
                                            account_id,
                                            to, "failed to send initial stream message: {e}"
                                        );
                                    },
                                }
                            }
                        },
                        Some(ts) => {
                            // Throttled edit-in-place.
                            if last_edit.elapsed() >= throttle {
                                let slack_text = markdown_to_slack(&accumulated);
                                // Truncate to Slack limit for in-progress edits.
                                let display = if slack_text.len() > SLACK_MAX_MESSAGE_LEN - 3 {
                                    format!(
                                        "{}...",
                                        &slack_text[..slack_text
                                            .floor_char_boundary(SLACK_MAX_MESSAGE_LEN - 3)]
                                    )
                                } else {
                                    format!("{slack_text}...")
                                };

                                if let Err(e) =
                                    update_message(&client, &token, to, ts, &display).await
                                {
                                    debug!(
                                        account_id,
                                        to, "stream edit-in-place failed (will retry): {e}"
                                    );
                                }
                                last_edit = tokio::time::Instant::now();
                            }
                        },
                    }
                },
                Some(StreamEvent::Done) => break,
                Some(StreamEvent::Error(e)) => {
                    accumulated.push_str(&format!("\n\n:warning: {e}"));
                    break;
                },
                None => break, // Channel closed.
            }
        }

        // Final message.
        if accumulated.is_empty() {
            return Ok(());
        }

        let final_text = markdown_to_slack(&accumulated);
        let chunks = chunk_message(&final_text, SLACK_MAX_MESSAGE_LEN);

        match &sent_ts {
            Some(ts) => {
                // Update the existing message with the first chunk.
                if let Some(first) = chunks.first()
                    && let Err(e) = update_message(&client, &token, to, ts, first).await
                {
                    warn!(account_id, to, "failed to finalize stream message: {e}");
                }
                // Send remaining chunks as new messages.
                for chunk in chunks.iter().skip(1) {
                    if let Err(e) =
                        post_message(&client, &token, to, chunk, thread_ts.as_deref()).await
                    {
                        warn!(account_id, to, "failed to send overflow chunk: {e}");
                    }
                }
            },
            None => {
                // Never sent initial message — send all chunks now.
                for chunk in &chunks {
                    if let Err(e) =
                        post_message(&client, &token, to, chunk, thread_ts.as_deref()).await
                    {
                        warn!(account_id, to, "failed to send stream message: {e}");
                    }
                }
            },
        }

        Ok(())
    }

    async fn is_stream_enabled(&self, account_id: &str) -> bool {
        self.get_stream_mode(account_id) == StreamMode::EditInPlace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_thread_ts_from_reply_to() {
        let accounts =
            std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let outbound = SlackOutbound {
            accounts: accounts.clone(),
        };
        // reply_to takes precedence.
        let ts = outbound.get_thread_ts("acct", "C123", Some("1234567.890"));
        assert_eq!(ts, Some("1234567.890".to_string()));
    }

    #[test]
    fn get_thread_ts_no_account() {
        let accounts =
            std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let outbound = SlackOutbound { accounts };
        let ts = outbound.get_thread_ts("acct", "C123", None);
        assert!(ts.is_none());
    }
}
