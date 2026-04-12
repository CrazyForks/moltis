//! Nostr relay subscription loop — inbound DM pipeline.
//!
//! Subscribes to kind:4 (NIP-04 encrypted DMs) addressed to the bot's pubkey.
//! Events flow through dedup, self-message filtering, access control, and
//! decryption before being dispatched to the gateway via `ChannelEventSink`.

use std::sync::Arc;

use {nostr_sdk::prelude::*, tokio::sync::RwLock, tokio_util::sync::CancellationToken};

use crate::{
    access::{self, AccessDenied},
    config::NostrAccountConfig,
    seen::SeenTracker,
};

/// Maximum plaintext message size in bytes.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Run the relay subscription loop for a single Nostr account.
///
/// Subscribes to NIP-04 DMs (kind:4) targeted at `bot_pubkey` and dispatches
/// inbound messages to the gateway. Runs until `cancel` is triggered.
pub async fn run_subscription_loop(
    client: Client,
    keys: Keys,
    config: Arc<RwLock<NostrAccountConfig>>,
    account_id: String,
    event_sink: Arc<dyn moltis_channels::ChannelEventSink>,
    cancel: CancellationToken,
) {
    let bot_pubkey = keys.public_key();
    let now_secs =
        u64::try_from(::time::OffsetDateTime::now_utc().unix_timestamp()).unwrap_or_default();
    let since = Timestamp::from(now_secs);

    let filter = Filter::new()
        .kind(Kind::EncryptedDirectMessage)
        .pubkey(bot_pubkey)
        .since(since);

    let npub = bot_pubkey
        .to_bech32()
        .unwrap_or_else(|_| bot_pubkey.to_hex());
    tracing::info!(
        account_id,
        pubkey = %npub,
        "starting Nostr DM subscription"
    );

    let mut seen = SeenTracker::new();

    // Subscribe (single filter)
    if let Err(e) = client.subscribe(filter, None).await {
        tracing::error!(account_id, "failed to subscribe: {e}");
        return;
    }

    let mut notifications = client.notifications();

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                tracing::info!(account_id, "Nostr subscription cancelled");
                break;
            }
            notification = notifications.recv() => {
                match notification {
                    Ok(RelayPoolNotification::Event { event, .. }) => {
                        handle_event(
                            &event,
                            &keys,
                            &bot_pubkey,
                            since,
                            &mut seen,
                            &config,
                            &account_id,
                            &event_sink,
                        ).await;
                    }
                    Ok(RelayPoolNotification::Shutdown) => {
                        tracing::warn!(account_id, "relay pool shutdown");
                        break;
                    }
                    Ok(_) => {} // Message, etc.
                    Err(e) => {
                        tracing::warn!(account_id, "notification channel error: {e}");
                    }
                }
            }
        }
    }

    tracing::info!(account_id, "Nostr subscription loop exited");
}

/// Process a single inbound event through the pipeline.
#[allow(clippy::too_many_arguments)]
async fn handle_event(
    event: &Event,
    keys: &Keys,
    bot_pubkey: &PublicKey,
    since: Timestamp,
    seen: &mut SeenTracker,
    config: &Arc<RwLock<NostrAccountConfig>>,
    account_id: &str,
    event_sink: &Arc<dyn moltis_channels::ChannelEventSink>,
) {
    // 1. Skip non-DM events
    if event.kind != Kind::EncryptedDirectMessage {
        return;
    }

    // 2. Dedup check
    if seen.check_and_insert(&event.id) {
        return;
    }

    // 3. Skip self-messages
    if event.pubkey == *bot_pubkey {
        return;
    }

    // 4. Skip stale events
    if event.created_at < since {
        return;
    }

    // 5. Access control (BEFORE decryption)
    let cfg = config.read().await;
    let access_result =
        access::check_dm_access(&event.pubkey, &cfg.dm_policy, &cfg.allowed_pubkeys);

    let sender_hex = event.pubkey.to_hex();
    let sender_npub = event
        .pubkey
        .to_bech32()
        .unwrap_or_else(|_| sender_hex.clone());

    match &access_result {
        Ok(()) => {},
        Err(AccessDenied::Disabled) => {
            tracing::debug!(account_id, sender = sender_hex, "DM rejected: disabled");
            return;
        },
        Err(AccessDenied::NotAllowlisted) => {
            if cfg.otp_self_approval {
                event_sink
                    .emit(moltis_channels::ChannelEvent::InboundMessage {
                        channel_type: moltis_channels::ChannelType::Nostr,
                        account_id: account_id.to_string(),
                        peer_id: sender_hex.clone(),
                        username: Some(sender_npub.clone()),
                        sender_name: None,
                        message_count: None,
                        access_granted: false,
                    })
                    .await;
                return;
            }
            tracing::debug!(
                account_id,
                sender = sender_hex,
                "DM rejected: not allowlisted"
            );
            return;
        },
    }
    drop(cfg);

    // 6. Decrypt content (NIP-04)
    let plaintext = match nip04::decrypt(keys.secret_key(), &event.pubkey, &event.content) {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(account_id, event_id = %event.id, "decrypt failed: {e}");
            return;
        },
    };

    // 7. Size validation
    let text = if plaintext.len() > MAX_MESSAGE_BYTES {
        tracing::warn!(account_id, len = plaintext.len(), "DM exceeds size limit");
        &plaintext[..MAX_MESSAGE_BYTES]
    } else {
        &plaintext
    };

    // 8. Emit inbound event
    event_sink
        .emit(moltis_channels::ChannelEvent::InboundMessage {
            channel_type: moltis_channels::ChannelType::Nostr,
            account_id: account_id.to_string(),
            peer_id: sender_hex.clone(),
            username: Some(sender_npub.clone()),
            sender_name: None,
            message_count: None,
            access_granted: true,
        })
        .await;

    // 9. Dispatch to gateway
    let reply_to = moltis_channels::ChannelReplyTarget {
        channel_type: moltis_channels::ChannelType::Nostr,
        account_id: account_id.to_string(),
        chat_id: sender_hex,
        message_id: Some(event.id.to_hex()),
        thread_id: None,
    };

    let meta = moltis_channels::ChannelMessageMeta {
        channel_type: moltis_channels::ChannelType::Nostr,
        sender_name: None,
        username: Some(sender_npub),
        message_kind: Some(moltis_channels::ChannelMessageKind::Text),
        model: None,
        audio_filename: None,
    };

    event_sink.dispatch_to_chat(text, reply_to, meta).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_message_size_is_reasonable() {
        assert_eq!(MAX_MESSAGE_BYTES, 64 * 1024);
    }
}
