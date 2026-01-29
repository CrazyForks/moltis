//! Session storage and management.
//!
//! Sessions are stored as JSONL files (one message per line) at
//! ~/.clawdbot/agents/<agentId>/sessions/<sessionKey>.jsonl
//! with file locking for concurrent access.

pub mod store;
pub mod compaction;
pub mod key;

pub use key::SessionKey;
