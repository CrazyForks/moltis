use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use tokio_util::sync::CancellationToken;

use moltis_channels::{ChannelEventSink, message_log::MessageLog};

use crate::{config::TelegramAccountConfig, outbound::TelegramOutbound};

/// Shared account state map.
pub type AccountStateMap = Arc<RwLock<HashMap<String, AccountState>>>;

/// Per-account runtime state.
pub struct AccountState {
    pub bot: teloxide::Bot,
    pub bot_username: Option<String>,
    pub account_id: String,
    pub config: TelegramAccountConfig,
    pub outbound: Arc<TelegramOutbound>,
    pub cancel: CancellationToken,
    pub message_log: Option<Arc<dyn MessageLog>>,
    pub event_sink: Option<Arc<dyn ChannelEventSink>>,
}
