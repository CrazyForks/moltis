//! Plugin system: discovery, loading, hooks, and API.
//!
//! Discovery paths: bundled (extensions/), global (~/.clawdbot/plugins/),
//! workspace (<workspace>/plugins/), config (plugins.external).

pub mod loader;
pub mod hooks;
pub mod api;
pub mod provider;
