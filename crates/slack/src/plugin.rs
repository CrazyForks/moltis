use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use {async_trait::async_trait, tracing::info};

use moltis_channels::{
    ChannelConfigView, Error as ChannelError, Result as ChannelResult,
    plugin::{ChannelOutbound, ChannelPlugin, ChannelStreamOutbound},
};

use crate::config::SlackAccountConfig;

/// In-memory state for a single Slack account.
struct AccountState {
    config: SlackAccountConfig,
}

type AccountStateMap = Arc<RwLock<HashMap<String, AccountState>>>;

/// Slack outbound message sender (stub).
struct SlackOutbound {
    accounts: AccountStateMap,
}

#[async_trait]
impl ChannelOutbound for SlackOutbound {
    async fn send_text(
        &self,
        account_id: &str,
        to: &str,
        text: &str,
        _reply_to: Option<&str>,
    ) -> ChannelResult<()> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        if !accounts.contains_key(account_id) {
            return Err(ChannelError::unknown_account(account_id));
        }
        tracing::debug!(account_id, to, "slack send_text stub: {text}");
        Ok(())
    }

    async fn send_media(
        &self,
        account_id: &str,
        to: &str,
        _payload: &moltis_common::types::ReplyPayload,
        _reply_to: Option<&str>,
    ) -> ChannelResult<()> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        if !accounts.contains_key(account_id) {
            return Err(ChannelError::unknown_account(account_id));
        }
        tracing::debug!(account_id, to, "slack send_media stub");
        Ok(())
    }
}

#[async_trait]
impl ChannelStreamOutbound for SlackOutbound {
    async fn send_stream(
        &self,
        account_id: &str,
        to: &str,
        _reply_to: Option<&str>,
        _stream: moltis_channels::StreamReceiver,
    ) -> ChannelResult<()> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        if !accounts.contains_key(account_id) {
            return Err(ChannelError::unknown_account(account_id));
        }
        tracing::debug!(account_id, to, "slack send_stream stub");
        Ok(())
    }
}

/// Slack channel plugin (skeleton).
pub struct SlackPlugin {
    accounts: AccountStateMap,
    outbound: SlackOutbound,
}

impl SlackPlugin {
    pub fn new() -> Self {
        let accounts: AccountStateMap = Arc::new(RwLock::new(HashMap::new()));
        let outbound = SlackOutbound {
            accounts: Arc::clone(&accounts),
        };
        Self { accounts, outbound }
    }
}

impl Default for SlackPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for SlackPlugin {
    fn id(&self) -> &str {
        "slack"
    }

    fn name(&self) -> &str {
        "Slack"
    }

    async fn start_account(
        &mut self,
        account_id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<()> {
        let slack_config: SlackAccountConfig = serde_json::from_value(config)?;
        info!(account_id, "starting Slack account (skeleton)");
        let mut accounts = self.accounts.write().unwrap_or_else(|e| e.into_inner());
        accounts.insert(account_id.to_string(), AccountState {
            config: slack_config,
        });
        Ok(())
    }

    async fn stop_account(&mut self, account_id: &str) -> ChannelResult<()> {
        info!(account_id, "stopping Slack account");
        let mut accounts = self.accounts.write().unwrap_or_else(|e| e.into_inner());
        accounts.remove(account_id);
        Ok(())
    }

    fn outbound(&self) -> Option<&dyn ChannelOutbound> {
        Some(&self.outbound)
    }

    fn status(&self) -> Option<&dyn moltis_channels::ChannelStatus> {
        None
    }

    fn has_account(&self, account_id: &str) -> bool {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts.contains_key(account_id)
    }

    fn account_ids(&self) -> Vec<String> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts.keys().cloned().collect()
    }

    fn account_config(&self, account_id: &str) -> Option<Box<dyn ChannelConfigView>> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_id)
            .map(|s| Box::new(s.config.clone()) as Box<dyn ChannelConfigView>)
    }

    fn update_account_config(
        &self,
        account_id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<()> {
        let slack_config: SlackAccountConfig = serde_json::from_value(config)?;
        let mut accounts = self.accounts.write().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = accounts.get_mut(account_id) {
            state.config = slack_config;
            Ok(())
        } else {
            Err(ChannelError::unknown_account(account_id))
        }
    }

    fn shared_outbound(&self) -> Arc<dyn ChannelOutbound> {
        Arc::new(SlackOutbound {
            accounts: Arc::clone(&self.accounts),
        })
    }

    fn shared_stream_outbound(&self) -> Arc<dyn ChannelStreamOutbound> {
        Arc::new(SlackOutbound {
            accounts: Arc::clone(&self.accounts),
        })
    }

    fn account_config_json(&self, account_id: &str) -> Option<serde_json::Value> {
        let accounts = self.accounts.read().unwrap_or_else(|e| e.into_inner());
        accounts
            .get(account_id)
            .and_then(|s| serde_json::to_value(&s.config).ok())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn plugin_id_and_name() {
        let plugin = SlackPlugin::new();
        assert_eq!(plugin.id(), "slack");
        assert_eq!(plugin.name(), "Slack");
    }

    #[test]
    fn empty_account_ids() {
        let plugin = SlackPlugin::new();
        assert!(plugin.account_ids().is_empty());
    }

    #[tokio::test]
    async fn start_and_stop_account() {
        let mut plugin = SlackPlugin::new();
        let config = serde_json::json!({ "token": "xoxb-test" });
        plugin.start_account("test", config).await.unwrap();
        assert!(plugin.has_account("test"));
        assert_eq!(plugin.account_ids(), vec!["test"]);

        plugin.stop_account("test").await.unwrap();
        assert!(!plugin.has_account("test"));
    }

    #[tokio::test]
    async fn account_config_round_trip() {
        let mut plugin = SlackPlugin::new();
        let config = serde_json::json!({
            "token": "xoxb-test",
            "dm_policy": "open",
            "allowlist": ["U123"]
        });
        plugin.start_account("bot1", config).await.unwrap();

        let view = plugin.account_config("bot1").unwrap();
        assert_eq!(view.allowlist(), &["U123"]);

        let json = plugin.account_config_json("bot1").unwrap();
        assert_eq!(json["token"], "xoxb-test");
    }

    #[test]
    fn update_config_unknown_account_errors() {
        let plugin = SlackPlugin::new();
        let result = plugin.update_account_config("nope", serde_json::json!({}));
        assert!(result.is_err());
    }
}
