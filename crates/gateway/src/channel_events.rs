use std::sync::Arc;

use {
    anyhow::anyhow,
    async_trait::async_trait,
    tracing::{debug, error, info, warn},
};

use moltis_channels::{ChannelEvent, ChannelEventSink, ChannelMessageMeta, ChannelReplyTarget};
use moltis_sessions::metadata::SqliteSessionMetadata;

use crate::{
    broadcast::{BroadcastOpts, broadcast},
    state::GatewayState,
};

/// Default (deterministic) session key for a channel chat.
fn default_channel_session_key(target: &ChannelReplyTarget) -> String {
    format!(
        "{}:{}:{}",
        target.channel_type, target.account_id, target.chat_id
    )
}

/// Resolve the active session key for a channel chat.
/// Uses the forward mapping table if an override exists, otherwise falls back
/// to the deterministic key.
async fn resolve_channel_session(
    target: &ChannelReplyTarget,
    metadata: &SqliteSessionMetadata,
) -> String {
    if let Some(key) = metadata
        .get_active_session(&target.channel_type, &target.account_id, &target.chat_id)
        .await
    {
        return key;
    }
    default_channel_session_key(target)
}

/// Broadcasts channel events over the gateway WebSocket.
///
/// Uses a deferred `OnceCell` reference so the sink can be created before
/// `GatewayState` exists (same pattern as cron callbacks).
pub struct GatewayChannelEventSink {
    state: Arc<tokio::sync::OnceCell<Arc<GatewayState>>>,
}

impl GatewayChannelEventSink {
    pub fn new(state: Arc<tokio::sync::OnceCell<Arc<GatewayState>>>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ChannelEventSink for GatewayChannelEventSink {
    async fn emit(&self, event: ChannelEvent) {
        if let Some(state) = self.state.get() {
            let payload = match serde_json::to_value(&event) {
                Ok(v) => v,
                Err(e) => {
                    warn!("failed to serialize channel event: {e}");
                    return;
                },
            };
            broadcast(state, "channel", payload, BroadcastOpts {
                drop_if_slow: true,
                ..Default::default()
            })
            .await;
        }
    }

    async fn dispatch_to_chat(
        &self,
        text: &str,
        reply_to: ChannelReplyTarget,
        meta: ChannelMessageMeta,
    ) {
        if let Some(state) = self.state.get() {
            let session_key = if let Some(ref sm) = state.services.session_metadata {
                resolve_channel_session(&reply_to, sm).await
            } else {
                default_channel_session_key(&reply_to)
            };

            // Broadcast a "chat" event so the web UI shows the user message
            // in real-time (like typing from the UI).
            let payload = serde_json::json!({
                "state": "channel_user",
                "text": text,
                "channel": &meta,
                "sessionKey": &session_key,
            });
            broadcast(state, "chat", payload, BroadcastOpts {
                drop_if_slow: true,
                ..Default::default()
            })
            .await;

            // Register the reply target so the chat "final" broadcast can
            // route the response back to the originating channel.
            state
                .push_channel_reply(&session_key, reply_to.clone())
                .await;

            // Persist channel binding so web UI messages on this session
            // can be echoed back to the channel.
            if let Ok(binding_json) = serde_json::to_string(&reply_to)
                && let Some(ref session_meta) = state.services.session_metadata
            {
                // Label the first session "Telegram 1" if it has no label yet.
                if let Some(entry) = session_meta.get(&session_key).await
                    && entry.channel_binding.is_none()
                {
                    let existing = session_meta
                        .list_channel_sessions(
                            &reply_to.channel_type,
                            &reply_to.account_id,
                            &reply_to.chat_id,
                        )
                        .await;
                    let n = existing.len() + 1;
                    let _ = session_meta
                        .upsert(&session_key, Some(format!("Telegram {n}")))
                        .await;
                }
                session_meta
                    .set_channel_binding(&session_key, Some(binding_json))
                    .await;
            }

            let chat = state.chat().await;
            let mut params = serde_json::json!({
                "text": text,
                "channel": &meta,
                "_session_key": &session_key,
            });
            // Forward the channel's default model to chat.send() if configured.
            if let Some(ref model) = meta.model {
                params["model"] = serde_json::json!(model);
            }

            // Send a repeating "typing" indicator every 4s until chat.send()
            // completes. Telegram's typing status expires after ~5s.
            if let Some(outbound) = state.services.channel_outbound_arc() {
                let (done_tx, mut done_rx) = tokio::sync::oneshot::channel::<()>();
                let account_id = reply_to.account_id.clone();
                let chat_id = reply_to.chat_id.clone();
                tokio::spawn(async move {
                    loop {
                        if let Err(e) = outbound.send_typing(&account_id, &chat_id).await {
                            debug!("typing indicator failed: {e}");
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_secs(4)) => {},
                            _ = &mut done_rx => break,
                        }
                    }
                });
                if let Err(e) = chat.send(params).await {
                    error!("channel dispatch_to_chat failed: {e}");
                }
                let _ = done_tx.send(());
            } else if let Err(e) = chat.send(params).await {
                error!("channel dispatch_to_chat failed: {e}");
            }
        } else {
            warn!("channel dispatch_to_chat: gateway not ready");
        }
    }

    async fn dispatch_command(
        &self,
        command: &str,
        reply_to: ChannelReplyTarget,
    ) -> anyhow::Result<String> {
        let state = self
            .state
            .get()
            .ok_or_else(|| anyhow!("gateway not ready"))?;
        let session_metadata = state
            .services
            .session_metadata
            .as_ref()
            .ok_or_else(|| anyhow!("session metadata not available"))?;
        let session_key = resolve_channel_session(&reply_to, session_metadata).await;
        let chat = state.chat().await;

        // Extract the command name (first word) and args (rest).
        let cmd = command.split_whitespace().next().unwrap_or("");
        let args = command[cmd.len()..].trim();

        match cmd {
            "new" => {
                // Create a new session with a fresh UUID key.
                let new_key = format!("session:{}", uuid::Uuid::new_v4());
                let binding_json = serde_json::to_string(&reply_to)
                    .map_err(|e| anyhow!("failed to serialize binding: {e}"))?;

                // Sequential label: count existing sessions for this chat.
                let existing = session_metadata
                    .list_channel_sessions(
                        &reply_to.channel_type,
                        &reply_to.account_id,
                        &reply_to.chat_id,
                    )
                    .await;
                let n = existing.len() + 1;

                // Create the new session entry with channel binding.
                session_metadata
                    .upsert(&new_key, Some(format!("Telegram {n}")))
                    .await
                    .map_err(|e| anyhow!("failed to create session: {e}"))?;
                session_metadata
                    .set_channel_binding(&new_key, Some(binding_json.clone()))
                    .await;

                // Ensure the old session also has a channel binding (for listing).
                let old_entry = session_metadata.get(&session_key).await;
                if old_entry
                    .as_ref()
                    .and_then(|e| e.channel_binding.as_ref())
                    .is_none()
                {
                    session_metadata
                        .set_channel_binding(&session_key, Some(binding_json))
                        .await;
                }

                // Update forward mapping.
                session_metadata
                    .set_active_session(
                        &reply_to.channel_type,
                        &reply_to.account_id,
                        &reply_to.chat_id,
                        &new_key,
                    )
                    .await;

                info!(
                    old_session = %session_key,
                    new_session = %new_key,
                    "channel /new: created new session"
                );

                // Notify web UI so the session list refreshes.
                broadcast(
                    state,
                    "session",
                    serde_json::json!({
                        "kind": "created",
                        "sessionKey": &new_key,
                    }),
                    BroadcastOpts {
                        drop_if_slow: true,
                        ..Default::default()
                    },
                )
                .await;

                Ok("New session started.".to_string())
            },
            "clear" => {
                let params = serde_json::json!({ "_session_key": &session_key });
                chat.clear(params).await.map_err(|e| anyhow!("{e}"))?;
                Ok("Session cleared.".to_string())
            },
            "compact" => {
                let params = serde_json::json!({ "_session_key": &session_key });
                chat.compact(params).await.map_err(|e| anyhow!("{e}"))?;
                Ok("Session compacted.".to_string())
            },
            "context" => {
                let params = serde_json::json!({ "_session_key": &session_key });
                let res = chat.context(params).await.map_err(|e| anyhow!("{e}"))?;
                let session_info = res.get("session").cloned().unwrap_or_default();
                let msg_count = session_info
                    .get("messageCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let model = session_info
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let tokens = res.get("tokenUsage").cloned().unwrap_or_default();
                let estimated = tokens
                    .get("estimatedTotal")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let context_window = tokens
                    .get("contextWindow")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                Ok(format!(
                    "Session: {session_key}\nMessages: {msg_count}\nModel: {model}\nTokens: ~{estimated}/{context_window}"
                ))
            },
            "sessions" => {
                let sessions = session_metadata
                    .list_channel_sessions(
                        &reply_to.channel_type,
                        &reply_to.account_id,
                        &reply_to.chat_id,
                    )
                    .await;

                if sessions.is_empty() {
                    return Ok("No sessions found. Send a message to start one.".to_string());
                }

                if args.is_empty() {
                    // List mode.
                    let mut lines = Vec::new();
                    for (i, s) in sessions.iter().enumerate() {
                        let label = s
                            .label
                            .as_deref()
                            .unwrap_or(&s.key);
                        let marker = if s.key == session_key { " *" } else { "" };
                        lines.push(format!(
                            "{}. {} ({} msgs){}",
                            i + 1,
                            label,
                            s.message_count,
                            marker,
                        ));
                    }
                    lines.push("\nUse /sessions N to switch.".to_string());
                    Ok(lines.join("\n"))
                } else {
                    // Switch mode.
                    let n: usize = args
                        .parse()
                        .map_err(|_| anyhow!("usage: /sessions [number]"))?;
                    if n == 0 || n > sessions.len() {
                        return Err(anyhow!(
                            "invalid session number. Use 1–{}.",
                            sessions.len()
                        ));
                    }
                    let target_session = &sessions[n - 1];

                    // Update forward mapping.
                    session_metadata
                        .set_active_session(
                            &reply_to.channel_type,
                            &reply_to.account_id,
                            &reply_to.chat_id,
                            &target_session.key,
                        )
                        .await;

                    let label = target_session
                        .label
                        .as_deref()
                        .unwrap_or(&target_session.key);
                    info!(
                        session = %target_session.key,
                        "channel /sessions: switched session"
                    );

                    broadcast(
                        state,
                        "session",
                        serde_json::json!({
                            "kind": "switched",
                            "sessionKey": &target_session.key,
                        }),
                        BroadcastOpts {
                            drop_if_slow: true,
                            ..Default::default()
                        },
                    )
                    .await;

                    Ok(format!("Switched to: {label}"))
                }
            },
            _ => Err(anyhow!("unknown command: /{cmd}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_event_serialization() {
        let event = ChannelEvent::InboundMessage {
            channel_type: "telegram".into(),
            account_id: "bot1".into(),
            peer_id: "123".into(),
            username: Some("alice".into()),
            sender_name: Some("Alice".into()),
            message_count: Some(5),
            access_granted: true,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "inbound_message");
        assert_eq!(json["channel_type"], "telegram");
        assert_eq!(json["account_id"], "bot1");
        assert_eq!(json["peer_id"], "123");
        assert_eq!(json["username"], "alice");
        assert_eq!(json["sender_name"], "Alice");
        assert_eq!(json["message_count"], 5);
        assert_eq!(json["access_granted"], true);
    }

    #[test]
    fn channel_session_key_format() {
        let target = ChannelReplyTarget {
            channel_type: "telegram".into(),
            account_id: "bot1".into(),
            chat_id: "12345".into(),
        };
        assert_eq!(default_channel_session_key(&target), "telegram:bot1:12345");
    }

    #[test]
    fn channel_session_key_group() {
        let target = ChannelReplyTarget {
            channel_type: "telegram".into(),
            account_id: "bot1".into(),
            chat_id: "-100999".into(),
        };
        assert_eq!(default_channel_session_key(&target), "telegram:bot1:-100999");
    }

    #[test]
    fn channel_event_serialization_nulls() {
        let event = ChannelEvent::InboundMessage {
            channel_type: "telegram".into(),
            account_id: "bot1".into(),
            peer_id: "123".into(),
            username: None,
            sender_name: None,
            message_count: None,
            access_granted: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["kind"], "inbound_message");
        assert!(json["username"].is_null());
        assert_eq!(json["access_granted"], false);
    }
}
