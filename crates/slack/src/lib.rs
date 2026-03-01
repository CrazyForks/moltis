//! Slack channel plugin (skeleton).
//!
//! Minimal implementation to prove the channel registry pattern:
//! adding a new channel requires only this crate + one `registry.register()`
//! call in the gateway — zero changes to `channel.rs`, `ChannelsConfig`, or
//! `ChannelType`.

pub mod config;
pub mod plugin;

pub use plugin::SlackPlugin;
