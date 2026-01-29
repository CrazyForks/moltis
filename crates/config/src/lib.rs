//! Configuration loading, validation, env substitution, includes, and legacy migration.
//!
//! Config file: ~/.clawdbot/config.json5
//! Supports JSON5 (comments, trailing commas), ${ENV_VAR} substitution,
//! $include directives, and auto-migration from old schemas.

pub mod loader;
pub mod schema;
pub mod env_subst;
pub mod migrate;
