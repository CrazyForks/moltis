use {
    moltis_channels::gating::{self, DmPolicy, GroupPolicy, MentionMode},
    moltis_common::types::ChatType,
};

use crate::config::TelegramAccountConfig;

/// Determine if an inbound message should be processed.
///
/// Returns `Ok(())` if the message is allowed, or `Err(reason)` if it should
/// be silently dropped.
pub fn check_access(
    config: &TelegramAccountConfig,
    chat_type: &ChatType,
    peer_id: &str,
    username: Option<&str>,
    group_id: Option<&str>,
    bot_mentioned: bool,
) -> Result<(), AccessDenied> {
    match chat_type {
        ChatType::Dm => check_dm_access(config, peer_id, username),
        ChatType::Group | ChatType::Channel => {
            check_group_access(config, peer_id, group_id, bot_mentioned)
        },
    }
}

fn check_dm_access(
    config: &TelegramAccountConfig,
    peer_id: &str,
    username: Option<&str>,
) -> Result<(), AccessDenied> {
    match config.dm_policy {
        DmPolicy::Disabled => Err(AccessDenied::DmsDisabled),
        DmPolicy::Open => Ok(()),
        DmPolicy::Allowlist => {
            if gating::is_allowed(peer_id, &config.allowlist)
                || username.is_some_and(|u| gating::is_allowed(u, &config.allowlist))
            {
                Ok(())
            } else {
                Err(AccessDenied::NotOnAllowlist)
            }
        },
    }
}

fn check_group_access(
    config: &TelegramAccountConfig,
    _peer_id: &str,
    group_id: Option<&str>,
    bot_mentioned: bool,
) -> Result<(), AccessDenied> {
    match config.group_policy {
        GroupPolicy::Disabled => return Err(AccessDenied::GroupsDisabled),
        GroupPolicy::Allowlist => {
            let gid = group_id.unwrap_or("");
            if !gating::is_allowed(gid, &config.group_allowlist) {
                return Err(AccessDenied::GroupNotOnAllowlist);
            }
        },
        GroupPolicy::Open => {},
    }

    // Mention gating
    match config.mention_mode {
        MentionMode::Always => Ok(()),
        MentionMode::None => Err(AccessDenied::MentionModeNone),
        MentionMode::Mention => {
            if bot_mentioned {
                Ok(())
            } else {
                Err(AccessDenied::NotMentioned)
            }
        },
    }
}

/// Reason an inbound message was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDenied {
    DmsDisabled,
    NotOnAllowlist,
    GroupsDisabled,
    GroupNotOnAllowlist,
    MentionModeNone,
    NotMentioned,
}

impl std::fmt::Display for AccessDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DmsDisabled => write!(f, "DMs are disabled"),
            Self::NotOnAllowlist => write!(f, "user not on allowlist"),
            Self::GroupsDisabled => write!(f, "groups are disabled"),
            Self::GroupNotOnAllowlist => write!(f, "group not on allowlist"),
            Self::MentionModeNone => write!(f, "bot does not respond in groups"),
            Self::NotMentioned => write!(f, "bot was not mentioned"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TelegramAccountConfig {
        TelegramAccountConfig::default()
    }

    #[test]
    fn open_dm_allows_all() {
        let c = cfg();
        assert!(check_access(&c, &ChatType::Dm, "anyone", None, None, false).is_ok());
    }

    #[test]
    fn disabled_dm_rejects() {
        let mut c = cfg();
        c.dm_policy = DmPolicy::Disabled;
        assert_eq!(
            check_access(&c, &ChatType::Dm, "user", None, None, false),
            Err(AccessDenied::DmsDisabled)
        );
    }

    #[test]
    fn allowlist_dm_by_peer_id() {
        let mut c = cfg();
        c.dm_policy = DmPolicy::Allowlist;
        c.allowlist = vec!["alice".into()];
        assert!(check_access(&c, &ChatType::Dm, "alice", None, None, false).is_ok());
        assert_eq!(
            check_access(&c, &ChatType::Dm, "bob", None, None, false),
            Err(AccessDenied::NotOnAllowlist)
        );
    }

    #[test]
    fn allowlist_dm_by_username() {
        let mut c = cfg();
        c.dm_policy = DmPolicy::Allowlist;
        c.allowlist = vec!["fabienpenso".into()];
        // Numeric peer_id doesn't match, but username does
        assert!(
            check_access(
                &c,
                &ChatType::Dm,
                "377114917",
                Some("fabienpenso"),
                None,
                false
            )
            .is_ok()
        );
        // Neither matches
        assert_eq!(
            check_access(&c, &ChatType::Dm, "377114917", Some("other"), None, false),
            Err(AccessDenied::NotOnAllowlist)
        );
        // No username provided, peer_id doesn't match
        assert_eq!(
            check_access(&c, &ChatType::Dm, "377114917", None, None, false),
            Err(AccessDenied::NotOnAllowlist)
        );
    }

    #[test]
    fn group_mention_required() {
        let c = cfg(); // mention_mode=Mention by default
        assert_eq!(
            check_access(&c, &ChatType::Group, "user", None, Some("grp1"), false),
            Err(AccessDenied::NotMentioned)
        );
        assert!(check_access(&c, &ChatType::Group, "user", None, Some("grp1"), true).is_ok());
    }

    #[test]
    fn group_always_mode() {
        let mut c = cfg();
        c.mention_mode = MentionMode::Always;
        assert!(check_access(&c, &ChatType::Group, "user", None, Some("grp1"), false).is_ok());
    }

    #[test]
    fn group_disabled() {
        let mut c = cfg();
        c.group_policy = GroupPolicy::Disabled;
        assert_eq!(
            check_access(&c, &ChatType::Group, "user", None, Some("grp1"), true),
            Err(AccessDenied::GroupsDisabled)
        );
    }

    #[test]
    fn group_allowlist() {
        let mut c = cfg();
        c.group_policy = GroupPolicy::Allowlist;
        c.group_allowlist = vec!["grp1".into()];
        c.mention_mode = MentionMode::Always;
        assert!(check_access(&c, &ChatType::Group, "user", None, Some("grp1"), false).is_ok());
        assert_eq!(
            check_access(&c, &ChatType::Group, "user", None, Some("grp2"), false),
            Err(AccessDenied::GroupNotOnAllowlist)
        );
    }
}
