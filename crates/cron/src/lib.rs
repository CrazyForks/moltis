//! Scheduled agent runs with cron expressions.
//! Persistent storage at ~/.clawdbot/cron-jobs.json.
//! Isolated agent execution (no session), optional delivery to a channel.

pub mod service;
pub mod store;
