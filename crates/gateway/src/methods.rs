use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::{debug, warn};

use moltis_protocol::{error_codes, ErrorShape, ResponseFrame};

use crate::state::GatewayState;

// ── Types ────────────────────────────────────────────────────────────────────

/// Context passed to every method handler.
pub struct MethodContext {
    pub request_id: String,
    pub method: String,
    pub params: serde_json::Value,
    pub client_conn_id: String,
    pub client_role: String,
    pub client_scopes: Vec<String>,
    pub state: Arc<GatewayState>,
}

/// The result a method handler produces.
pub type MethodResult = Result<serde_json::Value, ErrorShape>;

/// A boxed async method handler.
pub type HandlerFn = Box<
    dyn Fn(MethodContext) -> Pin<Box<dyn Future<Output = MethodResult> + Send>> + Send + Sync,
>;

// ── Scope authorization ──────────────────────────────────────────────────────

/// Methods that only the `node` role can call.
const NODE_METHODS: &[&str] = &["node.invoke.result", "node.event", "skills.bins"];

/// Methods requiring `operator.read` (or higher).
const READ_METHODS: &[&str] = &[
    "health",
    "logs.tail",
    "channels.status",
    "status",
    "usage.status",
    "usage.cost",
    "tts.status",
    "tts.providers",
    "models.list",
    "agents.list",
    "agent.identity.get",
    "skills.status",
    "voicewake.get",
    "sessions.list",
    "sessions.preview",
    "cron.list",
    "cron.status",
    "cron.runs",
    "system-presence",
    "last-heartbeat",
    "node.list",
    "node.describe",
    "chat.history",
];

/// Methods requiring `operator.write`.
const WRITE_METHODS: &[&str] = &[
    "send",
    "agent",
    "agent.wait",
    "wake",
    "talk.mode",
    "tts.enable",
    "tts.disable",
    "tts.convert",
    "tts.setProvider",
    "voicewake.set",
    "node.invoke",
    "chat.send",
    "chat.abort",
    "browser.request",
];

/// Methods requiring `operator.approvals`.
const APPROVAL_METHODS: &[&str] = &["exec.approval.request", "exec.approval.resolve"];

/// Methods requiring `operator.pairing`.
const PAIRING_METHODS: &[&str] = &[
    "node.pair.request",
    "node.pair.list",
    "node.pair.approve",
    "node.pair.reject",
    "node.pair.verify",
    "device.pair.list",
    "device.pair.approve",
    "device.pair.reject",
    "device.token.rotate",
    "device.token.revoke",
    "node.rename",
];

fn is_in(method: &str, list: &[&str]) -> bool {
    list.contains(&method)
}

/// Check role + scopes for a method. Returns None if authorized, Some(error) if not.
pub fn authorize_method(method: &str, role: &str, scopes: &[String]) -> Option<ErrorShape> {
    use moltis_protocol::scopes as s;

    // Node role: only node-specific methods.
    if is_in(method, NODE_METHODS) {
        if role == "node" {
            return None;
        }
        return Some(ErrorShape::new(
            error_codes::INVALID_REQUEST,
            format!("unauthorized role: {role}"),
        ));
    }
    if role == "node" {
        return Some(ErrorShape::new(
            error_codes::INVALID_REQUEST,
            format!("unauthorized role: {role}"),
        ));
    }
    if role != "operator" {
        return Some(ErrorShape::new(
            error_codes::INVALID_REQUEST,
            format!("unauthorized role: {role}"),
        ));
    }

    let has = |scope: &str| scopes.iter().any(|s| s == scope);

    // Admin scope grants everything.
    if has(s::ADMIN) {
        return None;
    }

    if is_in(method, APPROVAL_METHODS) && !has(s::APPROVALS) {
        return Some(ErrorShape::new(
            error_codes::INVALID_REQUEST,
            "missing scope: operator.approvals",
        ));
    }
    if is_in(method, PAIRING_METHODS) && !has(s::PAIRING) {
        return Some(ErrorShape::new(
            error_codes::INVALID_REQUEST,
            "missing scope: operator.pairing",
        ));
    }
    if is_in(method, READ_METHODS) && !(has(s::READ) || has(s::WRITE)) {
        return Some(ErrorShape::new(
            error_codes::INVALID_REQUEST,
            "missing scope: operator.read",
        ));
    }
    if is_in(method, WRITE_METHODS) && !has(s::WRITE) {
        return Some(ErrorShape::new(
            error_codes::INVALID_REQUEST,
            "missing scope: operator.write",
        ));
    }

    // Known authorized methods — pass through.
    if is_in(method, APPROVAL_METHODS)
        || is_in(method, PAIRING_METHODS)
        || is_in(method, READ_METHODS)
        || is_in(method, WRITE_METHODS)
    {
        return None;
    }

    // Everything else requires admin.
    Some(ErrorShape::new(
        error_codes::INVALID_REQUEST,
        "missing scope: operator.admin",
    ))
}

// ── Method registry ──────────────────────────────────────────────────────────

pub struct MethodRegistry {
    handlers: HashMap<String, HandlerFn>,
}

impl Default for MethodRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MethodRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            handlers: HashMap::new(),
        };
        reg.register_defaults();
        reg
    }

    /// Register a method handler.
    pub fn register(&mut self, method: impl Into<String>, handler: HandlerFn) {
        self.handlers.insert(method.into(), handler);
    }

    /// Dispatch a request: authorize, look up handler, call, return response frame.
    pub async fn dispatch(
        &self,
        ctx: MethodContext,
    ) -> ResponseFrame {
        let method = ctx.method.clone();
        let request_id = ctx.request_id.clone();
        let conn_id = ctx.client_conn_id.clone();

        // Authorization check.
        if let Some(err) = authorize_method(&method, &ctx.client_role, &ctx.client_scopes) {
            warn!(method, conn_id = %conn_id, code = %err.code, "method auth denied");
            return ResponseFrame::err(&request_id, err);
        }

        let Some(handler) = self.handlers.get(&method) else {
            warn!(method, conn_id = %conn_id, "unknown method");
            return ResponseFrame::err(
                &request_id,
                ErrorShape::new(error_codes::INVALID_REQUEST, format!("unknown method: {method}")),
            );
        };

        debug!(method, request_id = %request_id, conn_id = %conn_id, "dispatching method");
        match handler(ctx).await {
            Ok(payload) => {
                debug!(method, request_id = %request_id, "method ok");
                ResponseFrame::ok(&request_id, payload)
            }
            Err(err) => {
                warn!(method, request_id = %request_id, code = %err.code, msg = %err.message, "method error");
                ResponseFrame::err(&request_id, err)
            }
        }
    }

    /// List all registered method names.
    pub fn method_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.handlers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Register stub handlers for core gateway methods.
    fn register_defaults(&mut self) {
        // Health — the only method with real logic for now.
        self.register(
            "health",
            Box::new(|ctx| {
                Box::pin(async move {
                    let count = ctx.state.client_count().await;
                    Ok(serde_json::json!({
                        "status": "ok",
                        "version": ctx.state.version,
                        "connections": count,
                    }))
                })
            }),
        );

        // Status.
        self.register(
            "status",
            Box::new(|ctx| {
                Box::pin(async move {
                    Ok(serde_json::json!({
                        "version": ctx.state.version,
                        "hostname": ctx.state.hostname,
                        "connections": ctx.state.client_count().await,
                    }))
                })
            }),
        );

        // Stubs — return empty/placeholder results so the method is known.
        let stub_methods = [
            "channels.status",
            "channels.logout",
            "agent",
            "agent.wait",
            "agent.identity.get",
            "send",
            "wake",
            "sessions.list",
            "sessions.preview",
            "sessions.resolve",
            "sessions.patch",
            "sessions.reset",
            "sessions.delete",
            "sessions.compact",
            "config.get",
            "config.set",
            "config.apply",
            "config.patch",
            "config.schema",
            "cron.list",
            "cron.status",
            "cron.add",
            "cron.update",
            "cron.remove",
            "cron.run",
            "cron.runs",
            "models.list",
            "agents.list",
            "skills.status",
            "skills.bins",
            "skills.install",
            "skills.update",
            "node.list",
            "node.describe",
            "node.invoke",
            "node.invoke.result",
            "node.event",
            "node.pair.request",
            "node.pair.list",
            "node.pair.approve",
            "node.pair.reject",
            "node.pair.verify",
            "node.rename",
            "device.pair.list",
            "device.pair.approve",
            "device.pair.reject",
            "device.token.rotate",
            "device.token.revoke",
            "exec.approvals.get",
            "exec.approvals.set",
            "exec.approvals.node.get",
            "exec.approvals.node.set",
            "exec.approval.request",
            "exec.approval.resolve",
            "logs.tail",
            "chat.history",
            "chat.send",
            "chat.abort",
            "chat.inject",
            "talk.mode",
            "tts.status",
            "tts.providers",
            "tts.enable",
            "tts.disable",
            "tts.convert",
            "tts.setProvider",
            "voicewake.get",
            "voicewake.set",
            "browser.request",
            "usage.status",
            "usage.cost",
            "update.run",
            "system-presence",
            "system-event",
            "last-heartbeat",
            "set-heartbeats",
            "wizard.start",
            "wizard.next",
            "wizard.cancel",
            "wizard.status",
            "web.login.start",
            "web.login.wait",
        ];

        for method in stub_methods {
            self.register(
                method,
                Box::new(|_ctx| {
                    Box::pin(async { Ok(serde_json::json!({ "stub": true })) })
                }),
            );
        }
    }
}
