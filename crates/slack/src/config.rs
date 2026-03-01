use {
    moltis_channels::{
        config_view::ChannelConfigView,
        gating::{DmPolicy, GroupPolicy, MentionMode},
    },
    secrecy::{ExposeSecret, Secret},
    serde::{Deserialize, Serialize},
};

/// Stream mode for Slack responses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamMode {
    /// Edit the placeholder message in-place as tokens arrive.
    #[default]
    EditInPlace,
    /// Disable streaming — send the full response once complete.
    Off,
}

/// Configuration for a single Slack bot account.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackAccountConfig {
    /// Bot user OAuth token (`xoxb-...`).
    #[serde(serialize_with = "serialize_secret")]
    pub bot_token: Secret<String>,

    /// App-level token for Socket Mode (`xapp-...`).
    #[serde(serialize_with = "serialize_secret")]
    pub app_token: Secret<String>,

    /// DM access policy.
    pub dm_policy: DmPolicy,

    /// Channel/group access policy.
    pub group_policy: GroupPolicy,

    /// Mention activation mode for channels.
    pub mention_mode: MentionMode,

    /// DM user allowlist (Slack user IDs).
    #[serde(default)]
    pub allowlist: Vec<String>,

    /// Channel allowlist (Slack channel IDs).
    #[serde(default)]
    pub channel_allowlist: Vec<String>,

    /// Default model for this account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Provider for the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    /// Stream mode for responses.
    pub stream_mode: StreamMode,

    /// Minimum milliseconds between edit-in-place updates.
    pub edit_throttle_ms: u64,

    /// Reply in threads (default: true).
    pub thread_replies: bool,
}

impl std::fmt::Debug for SlackAccountConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackAccountConfig")
            .field("bot_token", &"[REDACTED]")
            .field("app_token", &"[REDACTED]")
            .field("dm_policy", &self.dm_policy)
            .field("group_policy", &self.group_policy)
            .field("mention_mode", &self.mention_mode)
            .field("allowlist", &self.allowlist)
            .field("channel_allowlist", &self.channel_allowlist)
            .field("model", &self.model)
            .field("model_provider", &self.model_provider)
            .field("stream_mode", &self.stream_mode)
            .field("edit_throttle_ms", &self.edit_throttle_ms)
            .field("thread_replies", &self.thread_replies)
            .finish()
    }
}

impl Default for SlackAccountConfig {
    fn default() -> Self {
        Self {
            bot_token: Secret::new(String::new()),
            app_token: Secret::new(String::new()),
            dm_policy: DmPolicy::Allowlist,
            group_policy: GroupPolicy::Open,
            mention_mode: MentionMode::Mention,
            allowlist: Vec::new(),
            channel_allowlist: Vec::new(),
            model: None,
            model_provider: None,
            stream_mode: StreamMode::EditInPlace,
            edit_throttle_ms: 500,
            thread_replies: true,
        }
    }
}

impl ChannelConfigView for SlackAccountConfig {
    fn allowlist(&self) -> &[String] {
        &self.allowlist
    }

    fn group_allowlist(&self) -> &[String] {
        &self.channel_allowlist
    }

    fn dm_policy(&self) -> DmPolicy {
        self.dm_policy.clone()
    }

    fn group_policy(&self) -> GroupPolicy {
        self.group_policy.clone()
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn model_provider(&self) -> Option<&str> {
        self.model_provider.as_deref()
    }
}

fn serialize_secret<S: serde::Serializer>(
    secret: &Secret<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose_secret())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips() {
        let cfg = SlackAccountConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();
        let _: SlackAccountConfig = serde_json::from_value(json).unwrap();
    }

    #[test]
    fn config_view_defaults() {
        let cfg = SlackAccountConfig::default();
        assert!(cfg.allowlist().is_empty());
        assert!(cfg.group_allowlist().is_empty());
        assert_eq!(cfg.dm_policy(), DmPolicy::Allowlist);
        assert_eq!(cfg.group_policy(), GroupPolicy::Open);
        assert!(cfg.model().is_none());
        assert!(cfg.model_provider().is_none());
    }

    #[test]
    fn config_with_tokens_round_trip() {
        let json = serde_json::json!({
            "bot_token": "xoxb-test-token",
            "app_token": "xapp-test-token",
            "dm_policy": "open",
            "group_policy": "allowlist",
            "mention_mode": "always",
            "allowlist": ["U123", "U456"],
            "channel_allowlist": ["C789"],
            "model": "claude-sonnet-4-20250514",
            "model_provider": "anthropic",
            "stream_mode": "edit_in_place",
            "edit_throttle_ms": 300,
            "thread_replies": false,
        });
        let cfg: SlackAccountConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.bot_token.expose_secret(), "xoxb-test-token");
        assert_eq!(cfg.app_token.expose_secret(), "xapp-test-token");
        assert_eq!(cfg.dm_policy, DmPolicy::Open);
        assert_eq!(cfg.group_policy, GroupPolicy::Allowlist);
        assert_eq!(cfg.mention_mode, MentionMode::Always);
        assert_eq!(cfg.allowlist, vec!["U123", "U456"]);
        assert_eq!(cfg.channel_allowlist, vec!["C789"]);
        assert_eq!(cfg.model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(cfg.stream_mode, StreamMode::EditInPlace);
        assert_eq!(cfg.edit_throttle_ms, 300);
        assert!(!cfg.thread_replies);

        // Round-trip
        let value = serde_json::to_value(&cfg).unwrap();
        let _: SlackAccountConfig = serde_json::from_value(value).unwrap();
    }

    #[test]
    fn stream_mode_off() {
        let json = serde_json::json!({
            "bot_token": "xoxb-test",
            "app_token": "xapp-test",
            "stream_mode": "off",
        });
        let cfg: SlackAccountConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.stream_mode, StreamMode::Off);
    }

    #[test]
    fn debug_redacts_tokens() {
        let cfg = SlackAccountConfig {
            bot_token: Secret::new("super-secret-bot".into()),
            app_token: Secret::new("super-secret-app".into()),
            ..Default::default()
        };
        let debug = format!("{cfg:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("super-secret-bot"));
        assert!(!debug.contains("super-secret-app"));
    }

    #[test]
    fn defaults_are_sensible() {
        let cfg = SlackAccountConfig::default();
        assert_eq!(cfg.stream_mode, StreamMode::EditInPlace);
        assert_eq!(cfg.edit_throttle_ms, 500);
        assert!(cfg.thread_replies);
        assert_eq!(cfg.mention_mode, MentionMode::Mention);
    }
}
