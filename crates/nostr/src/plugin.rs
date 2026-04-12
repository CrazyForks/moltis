//! Nostr channel plugin — lifecycle, registration, and OTP provider.

use std::{collections::HashMap, sync::Arc};

use {
    async_trait::async_trait,
    moltis_channels::{
        ChannelEventSink, ChannelOtpProvider, ChannelOutbound, ChannelPlugin, ChannelStatus,
        ChannelStreamOutbound, Result as ChannelResult, config_view::ChannelConfigView,
        message_log::MessageLog, otp::OtpChallengeInfo, plugin::ChannelHealthSnapshot,
    },
    nostr_sdk::prelude::*,
    serde_json::Value,
    tokio::sync::RwLock,
    tokio_util::sync::CancellationToken,
};

use crate::{
    bus,
    config::NostrAccountConfig,
    keys,
    outbound::{AccountStateMap, NostrOutbound},
    profile,
    state::AccountState,
};

/// Nostr channel plugin.
pub struct NostrPlugin {
    accounts: AccountStateMap,
    outbound: NostrOutbound,
    message_log: Option<Arc<dyn MessageLog>>,
    event_sink: Option<Arc<dyn ChannelEventSink>>,
}

impl NostrPlugin {
    pub fn new() -> Self {
        let accounts: AccountStateMap = Arc::new(RwLock::new(HashMap::new()));
        Self {
            outbound: NostrOutbound {
                accounts: Arc::clone(&accounts),
            },
            accounts,
            message_log: None,
            event_sink: None,
        }
    }

    pub fn with_message_log(mut self, log: Arc<dyn MessageLog>) -> Self {
        self.message_log = Some(log);
        self
    }

    pub fn with_event_sink(mut self, sink: Arc<dyn ChannelEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// List pending OTP challenges for a specific account.
    fn otp_challenges(&self, account_id: &str) -> Vec<OtpChallengeInfo> {
        let accounts = self.accounts.blocking_read();
        accounts
            .get(account_id)
            .map(|state| state.otp.list_pending())
            .unwrap_or_default()
    }
}

impl Default for NostrPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for NostrPlugin {
    fn id(&self) -> &str {
        "nostr"
    }

    fn name(&self) -> &str {
        "Nostr"
    }

    async fn start_account(&mut self, account_id: &str, config: Value) -> ChannelResult<()> {
        let nostr_config: NostrAccountConfig = serde_json::from_value(config).map_err(|e| {
            moltis_channels::Error::invalid_input(format!("invalid nostr config: {e}"))
        })?;

        if !nostr_config.enabled {
            tracing::info!(account_id, "nostr account disabled, skipping");
            return Ok(());
        }

        // Parse keys
        let bot_keys = keys::derive_keys(&nostr_config.secret_key).map_err(|e| {
            moltis_channels::Error::invalid_input(format!("invalid secret key: {e}"))
        })?;

        let npub = bot_keys
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| bot_keys.public_key().to_hex());
        tracing::info!(
            account_id,
            pubkey = %npub,
            relays = ?nostr_config.relays,
            "starting Nostr account"
        );

        // Build nostr-sdk client
        let client = Client::new(bot_keys.clone());

        // Add relays
        for relay_url in &nostr_config.relays {
            if let Err(e) = client.add_relay(relay_url).await {
                tracing::warn!(account_id, relay = relay_url, "failed to add relay: {e}");
            }
        }

        // Connect to relays
        client.connect().await;

        // Publish profile if configured
        if let Some(ref prof) = nostr_config.profile
            && let Err(e) = profile::publish_profile(&client, prof).await
        {
            tracing::warn!(account_id, "failed to publish profile: {e}");
        }

        let cancel = CancellationToken::new();

        // Spawn subscription loop
        let event_sink = self
            .event_sink
            .clone()
            .ok_or_else(|| moltis_channels::Error::unavailable("event sink not configured"))?;

        let loop_client = client.clone();
        let loop_keys = bot_keys.clone();
        let loop_config = Arc::new(RwLock::new(nostr_config.clone()));
        let loop_account_id = account_id.to_string();
        let loop_cancel = cancel.clone();
        let loop_sink = Arc::clone(&event_sink);

        tokio::spawn(async move {
            bus::run_subscription_loop(
                loop_client,
                loop_keys,
                loop_config,
                loop_account_id,
                loop_sink,
                loop_cancel,
            )
            .await;
        });

        // Store account state
        let otp_cooldown = nostr_config.otp_cooldown_secs;
        let state = AccountState {
            client,
            keys: bot_keys,
            config: nostr_config,
            cancel,
            otp: moltis_channels::otp::OtpState::new(otp_cooldown),
        };

        self.accounts
            .write()
            .await
            .insert(account_id.to_string(), state);

        tracing::info!(account_id, "Nostr account started");
        Ok(())
    }

    async fn stop_account(&mut self, account_id: &str) -> ChannelResult<()> {
        let state = self.accounts.write().await.remove(account_id);
        if let Some(state) = state {
            state.cancel.cancel();
            state.client.disconnect().await;
            tracing::info!(account_id, "Nostr account stopped");
        }
        Ok(())
    }

    fn outbound(&self) -> Option<&dyn ChannelOutbound> {
        Some(&self.outbound)
    }

    fn status(&self) -> Option<&dyn ChannelStatus> {
        Some(self)
    }

    fn has_account(&self, account_id: &str) -> bool {
        self.accounts.blocking_read().contains_key(account_id)
    }

    fn account_ids(&self) -> Vec<String> {
        self.accounts.blocking_read().keys().cloned().collect()
    }

    fn account_config(&self, account_id: &str) -> Option<Box<dyn ChannelConfigView>> {
        let accounts = self.accounts.blocking_read();
        accounts
            .get(account_id)
            .map(|s| Box::new(s.config.clone()) as Box<dyn ChannelConfigView>)
    }

    fn account_config_json(&self, account_id: &str) -> Option<Value> {
        let accounts = self.accounts.blocking_read();
        accounts
            .get(account_id)
            .and_then(|s| serde_json::to_value(crate::config::RedactedConfig(&s.config)).ok())
    }

    fn update_account_config(&self, account_id: &str, config: Value) -> ChannelResult<()> {
        let new_config: NostrAccountConfig = serde_json::from_value(config).map_err(|e| {
            moltis_channels::Error::invalid_input(format!("invalid nostr config: {e}"))
        })?;
        let mut accounts = self.accounts.blocking_write();
        if let Some(state) = accounts.get_mut(account_id) {
            state.config = new_config;
        }
        Ok(())
    }

    fn shared_outbound(&self) -> Arc<dyn ChannelOutbound> {
        Arc::new(NostrOutbound {
            accounts: Arc::clone(&self.accounts),
        })
    }

    fn shared_stream_outbound(&self) -> Arc<dyn ChannelStreamOutbound> {
        Arc::new(NostrOutbound {
            accounts: Arc::clone(&self.accounts),
        })
    }

    fn as_otp_provider(&self) -> Option<&dyn ChannelOtpProvider> {
        Some(self)
    }
}

#[async_trait]
impl ChannelStatus for NostrPlugin {
    async fn probe(&self, account_id: &str) -> ChannelResult<ChannelHealthSnapshot> {
        let accounts = self.accounts.read().await;
        let state = accounts.get(account_id);

        match state {
            Some(state) => {
                let relays = state.client.relays().await;
                let connected_count = relays
                    .values()
                    .filter(|r| r.status() == RelayStatus::Connected)
                    .count();
                let total = relays.len();

                let npub = state
                    .keys
                    .public_key()
                    .to_bech32()
                    .unwrap_or_else(|_| state.keys.public_key().to_hex());
                Ok(ChannelHealthSnapshot {
                    connected: connected_count > 0,
                    account_id: account_id.to_string(),
                    details: Some(format!("{connected_count}/{total} relays connected")),
                    extra: Some(serde_json::json!({
                        "pubkey": npub,
                        "connected_relays": connected_count,
                        "total_relays": total,
                    })),
                })
            },
            None => Ok(ChannelHealthSnapshot {
                connected: false,
                account_id: account_id.to_string(),
                details: Some("account not found".to_string()),
                extra: None,
            }),
        }
    }
}

impl ChannelOtpProvider for NostrPlugin {
    fn pending_otp_challenges(&self, account_id: &str) -> Vec<OtpChallengeInfo> {
        self.otp_challenges(account_id)
    }
}
