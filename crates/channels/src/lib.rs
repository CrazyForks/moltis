//! Channel plugin system.
//!
//! Each channel (Telegram, Discord, Slack, WhatsApp, etc.) implements the
//! ChannelPlugin trait with sub-traits for config, auth, inbound/outbound
//! messaging, status, and gateway lifecycle.

pub mod plugin;
pub mod registry;
pub mod gating;

pub use plugin::{ChannelPlugin, ChannelOutbound, ChannelStatus};
