//! Core types and constants for node execution.
//!
//! This crate contains the shared types and constants used by the gateway
//! and other crates for remote node execution.

use serde::{Deserialize, Serialize};

/// Result of a remote command execution on a node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Environment variables that are safe to forward to a remote node.
pub const SAFE_ENV_ALLOWLIST: &[&str] = &["TERM", "LANG", "COLORTERM", "NO_COLOR", "FORCE_COLOR"];

/// Environment variable prefixes that are safe to forward.
pub const SAFE_ENV_PREFIX_ALLOWLIST: &[&str] = &["LC_"];

/// Environment variable patterns that must NEVER be forwarded to a remote node.
pub const BLOCKED_ENV_PREFIXES: &[&str] = &[
    "DYLD_",
    "LD_",
    "NODE_OPTIONS",
    "PYTHON",
    "PERL",
    "RUBYOPT",
    "SHELLOPTS",
    "PS4",
    // Security-sensitive keys
    "MOLTIS_",
    "OPENAI_",
    "ANTHROPIC_",
    "AWS_",
    "GOOGLE_",
    "AZURE_",
];

/// SSH node ID prefix.
pub const SSH_ID_PREFIX: &str = "ssh:";

/// SSH target ID prefix.
pub const SSH_TARGET_ID_PREFIX: &str = "ssh:target:";
