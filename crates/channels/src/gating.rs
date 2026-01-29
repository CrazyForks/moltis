/// Allowlist matching, mention gating, and command gating for channels.
/// Check if a peer is allowed to interact with the bot.
pub fn is_allowed(_peer_id: &str, _allowlist: &[String]) -> bool {
    todo!("match peer against allowlist patterns")
}

/// Mention activation mode for group chats.
#[derive(Debug, Clone)]
pub enum MentionMode {
    /// Bot must be @mentioned to respond.
    Mention,
    /// Bot responds to all messages.
    Always,
    /// Bot does not respond in groups.
    None,
}
