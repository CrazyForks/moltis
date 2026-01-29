use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Token,
    Password,
}

/// Resolved gateway auth configuration.
#[derive(Debug, Clone)]
pub struct ResolvedAuth {
    pub mode: AuthMode,
    pub token: Option<String>,
    pub password: Option<String>,
}

/// Result of an authentication attempt.
#[derive(Debug, Clone)]
pub struct AuthResult {
    pub ok: bool,
    pub method: Option<AuthMethod>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum AuthMethod {
    Token,
    Password,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Constant-time string comparison (prevents timing attacks).
fn safe_equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // XOR each byte and accumulate; any difference makes result non-zero.
    let diff = a
        .as_bytes()
        .iter()
        .zip(b.as_bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));
    diff == 0
}

pub fn is_loopback(ip: &str) -> bool {
    ip == "127.0.0.1"
        || ip.starts_with("127.")
        || ip == "::1"
        || ip.starts_with("::ffff:127.")
}

// ── Auth logic ───────────────────────────────────────────────────────────────

/// Resolve auth config from environment / config values.
pub fn resolve_auth(token: Option<String>, password: Option<String>) -> ResolvedAuth {
    let mode = if password.is_some() {
        AuthMode::Password
    } else {
        AuthMode::Token
    };
    ResolvedAuth {
        mode,
        token,
        password,
    }
}

/// Authenticate an incoming WebSocket connect request.
pub fn authorize_connect(
    auth: &ResolvedAuth,
    provided_token: Option<&str>,
    provided_password: Option<&str>,
    _remote_ip: Option<&str>,
) -> AuthResult {
    match auth.mode {
        AuthMode::Token => {
            let Some(expected) = auth.token.as_deref() else {
                return AuthResult {
                    ok: false,
                    method: None,
                    reason: Some("token_missing_config".into()),
                };
            };
            let Some(given) = provided_token else {
                return AuthResult {
                    ok: false,
                    method: None,
                    reason: Some("token_missing".into()),
                };
            };
            if !safe_equal(given, expected) {
                return AuthResult {
                    ok: false,
                    method: None,
                    reason: Some("token_mismatch".into()),
                };
            }
            AuthResult {
                ok: true,
                method: Some(AuthMethod::Token),
                reason: None,
            }
        }
        AuthMode::Password => {
            let Some(expected) = auth.password.as_deref() else {
                return AuthResult {
                    ok: false,
                    method: None,
                    reason: Some("password_missing_config".into()),
                };
            };
            let Some(given) = provided_password else {
                return AuthResult {
                    ok: false,
                    method: None,
                    reason: Some("password_missing".into()),
                };
            };
            if !safe_equal(given, expected) {
                return AuthResult {
                    ok: false,
                    method: None,
                    reason: Some("password_mismatch".into()),
                };
            }
            AuthResult {
                ok: true,
                method: Some(AuthMethod::Password),
                reason: None,
            }
        }
    }
}
