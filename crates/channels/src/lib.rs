//! Channel plugin system.
//!
//! Each channel (Telegram, Discord, Slack, WhatsApp, etc.) implements the
//! ChannelPlugin trait with sub-traits for config, auth, inbound/outbound
//! messaging, status, and gateway lifecycle.

pub mod config_view;
pub mod error;
pub mod gating;
pub mod message_log;
pub mod otp;
pub mod plugin;
pub mod registry;
pub mod store;

pub use {
    config_view::ChannelConfigView,
    error::{Error, Result},
    plugin::{
        ChannelAttachment, ChannelEvent, ChannelEventSink, ChannelHealthSnapshot,
        ChannelMessageKind, ChannelMessageMeta, ChannelOtpProvider, ChannelOutbound, ChannelPlugin,
        ChannelReplyTarget, ChannelStatus, ChannelStreamOutbound, ChannelType, StreamEvent,
        StreamReceiver, StreamSender,
    },
    registry::{ChannelRegistry, RegistryOutboundRouter},
};
