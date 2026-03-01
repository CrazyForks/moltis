use {
    moltis_channels::{
        config_view::ChannelConfigView,
        gating::{DmPolicy, GroupPolicy},
    },
    serde::{Deserialize, Serialize},
};

/// Configuration for a single Slack bot account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackAccountConfig {
    /// Bot user OAuth token (`xoxb-...`).
    pub token: String,
    /// DM user allowlist (Slack user IDs).
    #[serde(default)]
    pub allowlist: Vec<String>,
    /// Channel/group allowlist (Slack channel IDs).
    #[serde(default)]
    pub group_allowlist: Vec<String>,
    /// DM access policy.
    #[serde(default)]
    pub dm_policy: DmPolicy,
    /// Group access policy.
    #[serde(default)]
    pub group_policy: GroupPolicy,
    /// Default model for this account.
    pub model: Option<String>,
    /// Provider for the model.
    pub model_provider: Option<String>,
}

impl ChannelConfigView for SlackAccountConfig {
    fn allowlist(&self) -> &[String] {
        &self.allowlist
    }

    fn group_allowlist(&self) -> &[String] {
        &self.group_allowlist
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
}
