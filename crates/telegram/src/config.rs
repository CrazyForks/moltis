use {
    moltis_channels::gating::{DmPolicy, GroupPolicy, MentionMode},
    serde::{Deserialize, Serialize},
};

/// How streaming responses are delivered.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamMode {
    /// Edit a placeholder message in place as tokens arrive.
    #[default]
    EditInPlace,
    /// No streaming — send the final response as a single message.
    Off,
}

/// Configuration for a single Telegram bot account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramAccountConfig {
    /// Bot token from @BotFather.
    pub token: String,

    /// DM access policy.
    pub dm_policy: DmPolicy,

    /// Group access policy.
    pub group_policy: GroupPolicy,

    /// Mention activation mode for groups.
    pub mention_mode: MentionMode,

    /// User/peer allowlist for DMs.
    pub allowlist: Vec<String>,

    /// Group/chat ID allowlist.
    pub group_allowlist: Vec<String>,

    /// How streaming responses are delivered.
    pub stream_mode: StreamMode,

    /// Minimum interval between edit-in-place updates (ms).
    pub edit_throttle_ms: u64,

    /// Default model ID for this bot's sessions (e.g. "openai/gpt-4o").
    /// When set, channel messages use this model instead of the first registered provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Default for TelegramAccountConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::default(),
            mention_mode: MentionMode::default(),
            allowlist: Vec::new(),
            group_allowlist: Vec::new(),
            stream_mode: StreamMode::default(),
            edit_throttle_ms: 300,
            model: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = TelegramAccountConfig::default();
        assert_eq!(cfg.dm_policy, DmPolicy::Open);
        assert_eq!(cfg.group_policy, GroupPolicy::Open);
        assert_eq!(cfg.mention_mode, MentionMode::Mention);
        assert_eq!(cfg.stream_mode, StreamMode::EditInPlace);
        assert_eq!(cfg.edit_throttle_ms, 300);
    }

    #[test]
    fn deserialize_from_json() {
        let json = r#"{
            "token": "123:ABC",
            "dm_policy": "allowlist",
            "stream_mode": "off",
            "allowlist": ["user1", "user2"]
        }"#;
        let cfg: TelegramAccountConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.token, "123:ABC");
        assert_eq!(cfg.dm_policy, DmPolicy::Allowlist);
        assert_eq!(cfg.stream_mode, StreamMode::Off);
        assert_eq!(cfg.allowlist, vec!["user1", "user2"]);
        // defaults for unspecified fields
        assert_eq!(cfg.group_policy, GroupPolicy::Open);
    }

    #[test]
    fn serialize_roundtrip() {
        let cfg = TelegramAccountConfig {
            token: "tok".into(),
            dm_policy: DmPolicy::Disabled,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let cfg2: TelegramAccountConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg2.dm_policy, DmPolicy::Disabled);
        assert_eq!(cfg2.token, "tok");
    }
}
