//! HTTP server entry points, middleware stack, and router construction.
//!
//! This module contains the HTTP-specific layer of the moltis gateway:
//! `AppState`, router building, middleware, handlers, and server startup.
//! Core business logic lives in `moltis-gateway`; this crate depends on it
//! but never the reverse.

use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

#[cfg(feature = "ngrok")]
use std::sync::Weak;

use {
    axum::{
        Router,
        extract::{ConnectInfo, State, WebSocketUpgrade},
        http::StatusCode,
        response::{IntoResponse, Json},
        routing::get,
    },
    tower_http::{
        catch_panic::CatchPanicLayer,
        compression::CompressionLayer,
        cors::{AllowOrigin, Any, CorsLayer},
        request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
        sensitive_headers::SetSensitiveHeadersLayer,
        set_header::SetResponseHeaderLayer,
        trace::{DefaultOnRequest, DefaultOnResponse, TraceLayer},
    },
    tracing::{Level, info, warn},
};

use {moltis_channels::ChannelPlugin, moltis_protocol::TICK_INTERVAL_MS};

use moltis_sessions::session_events::{SessionEvent, SessionEventBus};

#[cfg(feature = "ngrok")]
use secrecy::ExposeSecret;

#[cfg(feature = "ngrok")]
use tokio_util::sync::CancellationToken;

use moltis_gateway::{
    auth,
    auth_webauthn::SharedWebAuthnRegistry,
    broadcast::{BroadcastOpts, broadcast, broadcast_tick},
    methods::MethodRegistry,
    server::{PreparedGatewayCore, prepare_gateway_core},
    state::GatewayState,
    update_check::{UPDATE_CHECK_INTERVAL, fetch_update_availability, resolve_releases_url},
};

use crate::{
    auth_routes::{AuthState, auth_router},
    ws::handle_connection,
};

mod handlers;
mod runtime;

use {handlers::*, runtime::*};

#[cfg(feature = "tailscale")]
use moltis_gateway::tailscale::{CliTailscaleManager, TailscaleManager, TailscaleMode};

#[cfg(feature = "tls")]
use moltis_tls::CertManager;

/// Options for tailscale serve/funnel passed from CLI flags.
#[cfg(feature = "tailscale")]
pub struct TailscaleOpts {
    pub mode: String,
    pub reset_on_exit: bool,
}

#[cfg(feature = "ngrok")]
#[derive(Clone, Debug)]
pub struct NgrokRuntimeStatus {
    pub public_url: String,
    pub passkey_warning: Option<String>,
}

#[cfg(feature = "ngrok")]
struct NgrokActiveTunnel {
    session: ngrok::Session,
    forwarder: ngrok::forwarder::Forwarder<ngrok::tunnel::HttpTunnel>,
    loopback_shutdown: CancellationToken,
    loopback_task: tokio::task::JoinHandle<()>,
    status: NgrokRuntimeStatus,
}

#[cfg(feature = "ngrok")]
struct NgrokControllerInner {
    gateway: Arc<GatewayState>,
    webauthn_registry: Option<SharedWebAuthnRegistry>,
    runtime: Arc<tokio::sync::RwLock<Option<NgrokRuntimeStatus>>>,
    app: tokio::sync::RwLock<Option<Router>>,
    active_tunnel: tokio::sync::Mutex<Option<NgrokActiveTunnel>>,
}

#[cfg(feature = "ngrok")]
#[derive(Clone)]
pub struct NgrokController {
    inner: Arc<NgrokControllerInner>,
}

#[cfg(feature = "ngrok")]
impl NgrokController {
    fn new(
        gateway: Arc<GatewayState>,
        webauthn_registry: Option<SharedWebAuthnRegistry>,
        runtime: Arc<tokio::sync::RwLock<Option<NgrokRuntimeStatus>>>,
    ) -> Self {
        Self {
            inner: Arc::new(NgrokControllerInner {
                gateway,
                webauthn_registry,
                runtime,
                app: tokio::sync::RwLock::new(None),
                active_tunnel: tokio::sync::Mutex::new(None),
            }),
        }
    }

    pub async fn configure_app(&self, app: Router) {
        let mut stored = self.inner.app.write().await;
        *stored = Some(app);
    }

    pub async fn apply(
        &self,
        ngrok_config: &moltis_config::NgrokConfig,
    ) -> anyhow::Result<Option<NgrokRuntimeStatus>> {
        self.stop().await?;

        if !ngrok_config.enabled {
            info!("ngrok tunnel disabled");
            return Ok(None);
        }

        let active_tunnel = self.start(ngrok_config).await?;
        let status = active_tunnel.status.clone();
        {
            let mut runtime = self.inner.runtime.write().await;
            *runtime = Some(status.clone());
        }
        {
            let mut active = self.inner.active_tunnel.lock().await;
            *active = Some(active_tunnel);
        }
        info!(url = %status.public_url, "ngrok tunnel started");
        Ok(Some(status))
    }

    async fn start(
        &self,
        ngrok_config: &moltis_config::NgrokConfig,
    ) -> anyhow::Result<NgrokActiveTunnel> {
        let app = {
            let stored = self.inner.app.read().await;
            stored.clone().ok_or_else(|| {
                anyhow::anyhow!("ngrok tunnel cannot start before the HTTP app is ready")
            })?
        };

        start_ngrok_tunnel(
            app,
            Arc::clone(&self.inner.gateway),
            self.inner.webauthn_registry.clone(),
            ngrok_config,
        )
        .await
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        use ngrok::prelude::TunnelCloser;

        let active_tunnel = {
            let mut active = self.inner.active_tunnel.lock().await;
            active.take()
        };

        let Some(mut active_tunnel) = active_tunnel else {
            let mut runtime = self.inner.runtime.write().await;
            *runtime = None;
            return Ok(());
        };

        let stopped_url = active_tunnel.status.public_url.clone();
        active_tunnel.loopback_shutdown.cancel();

        if let Err(error) = active_tunnel.forwarder.close().await {
            warn!(url = %stopped_url, %error, "failed to close ngrok tunnel");
        }
        if let Err(error) = active_tunnel.session.close().await {
            warn!(url = %stopped_url, %error, "failed to close ngrok session");
        }

        match active_tunnel.forwarder.join().await {
            Ok(Ok(())) => {},
            Ok(Err(error)) => {
                warn!(url = %stopped_url, %error, "ngrok tunnel forwarder exited with error");
            },
            Err(error) => {
                warn!(url = %stopped_url, %error, "ngrok tunnel join failed");
            },
        }

        match active_tunnel.loopback_task.await {
            Ok(()) => {},
            Err(error) => {
                warn!(url = %stopped_url, %error, "ngrok loopback server task join failed");
            },
        }

        let mut runtime = self.inner.runtime.write().await;
        *runtime = None;
        info!(url = %stopped_url, "ngrok tunnel stopped");
        Ok(())
    }
}

// ── Shared app state ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub gateway: Arc<GatewayState>,
    pub methods: Arc<MethodRegistry>,
    pub request_throttle: Arc<crate::request_throttle::RequestThrottle>,
    pub webauthn_registry: Option<SharedWebAuthnRegistry>,
    #[cfg(feature = "ngrok")]
    pub ngrok_controller_owner: Option<Arc<NgrokController>>,
    #[cfg(feature = "ngrok")]
    pub ngrok_controller: Weak<NgrokController>,
    #[cfg(feature = "ngrok")]
    pub ngrok_runtime: Arc<tokio::sync::RwLock<Option<NgrokRuntimeStatus>>>,
    #[cfg(feature = "push-notifications")]
    pub push_service: Option<Arc<moltis_gateway::push::PushService>>,
    #[cfg(feature = "graphql")]
    pub graphql_schema: moltis_graphql::MoltisSchema,
}

/// Function signature for adding extra routes (e.g. web-UI) to the gateway.
pub type RouteEnhancer = fn() -> Router<AppState>;

#[cfg(feature = "ngrok")]
type GatewayBase = (Router<AppState>, AppState, Arc<NgrokController>);

#[cfg(not(feature = "ngrok"))]
type GatewayBase = (Router<AppState>, AppState);

// ── Server startup ───────────────────────────────────────────────────────────

/// Build the API routes (shared between both build_gateway_app versions).
///
/// Auth is enforced by `auth_gate` middleware on the whole router — these
/// routes no longer carry their own auth layer.
/// Add feature-specific routes to API routes.
/// Build the CORS layer with dynamic host-based origin validation.
///
/// Instead of `allow_origin(Any)`, this validates the `Origin` header against the
/// request's `Host` header using the same `is_same_origin` logic as the WebSocket
/// CSWSH protection. This is secure for Docker/cloud deployments where the hostname
/// is unknown at build time — the server dynamically allows its own origin at
/// request time.
fn build_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(
            |origin: &axum::http::HeaderValue, parts: &axum::http::request::Parts| {
                let origin_str = origin.to_str().unwrap_or("");
                let host = parts
                    .headers
                    .get(axum::http::header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                is_same_origin(origin_str, host)
            },
        ))
        .allow_methods(Any)
        .allow_headers(Any)
}

/// 2 MiB global request body limit — sufficient for any JSON API payload, small
/// enough to limit abuse. The upload endpoint has its own 25 MiB limit.
const REQUEST_BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Apply the full middleware stack to the router.
///
/// Layer order (outermost → innermost for requests):
/// 1. `CatchPanicLayer` — converts handler panics to 500s
/// 2. `SetSensitiveHeadersLayer` — marks Authorization/Cookie as redacted
/// 3. `SetRequestIdLayer` — generates x-request-id before tracing
/// 4. `TraceLayer` (optional) — logs requests with redacted headers + request ID
/// 5. `CorsLayer` — handles preflight; logged by trace
/// 6. `PropagateRequestIdLayer` — copies x-request-id to response
/// 7. Security response headers — X-Content-Type-Options, X-Frame-Options, etc.
/// 8. `RequestBodyLimitLayer` — rejects oversized bodies
/// 9. `CompressionLayer` (innermost) — compresses response body
fn apply_middleware_stack<S>(
    router: Router<S>,
    cors: CorsLayer,
    http_request_logs: bool,
) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    use axum::http::{HeaderValue, header};

    // Inner layers: compression, body limit, security headers, request ID propagation.
    let router = router
        .layer(CompressionLayer::new())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            REQUEST_BODY_LIMIT,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("sameorigin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(self), geolocation=(), payment=()"),
        ))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(cors);

    // Optional trace layer — sees redacted headers and request ID.
    let router = apply_http_trace_layer(router, http_request_logs);

    // Outer layers: request ID generation, sensitive header marking, panic catching.
    router
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(SetSensitiveHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
            header::SET_COOKIE,
        ]))
        .layer(CatchPanicLayer::new())
}

/// Apply optional HTTP request/response tracing layer.
fn apply_http_trace_layer<S>(router: Router<S>, enabled: bool) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    if enabled {
        let http_trace = TraceLayer::new_for_http()
            .make_span_with(|request: &axum::http::Request<_>| {
                let request_id = request
                    .headers()
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-")
                    .to_owned();
                let user_agent = request
                    .headers()
                    .get(axum::http::header::USER_AGENT)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-")
                    .to_owned();
                let referer = request
                    .headers()
                    .get(axum::http::header::REFERER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-")
                    .to_owned();
                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    uri = %request.uri(),
                    request_id = %request_id,
                    user_agent = %user_agent,
                    referer = %referer
                )
            })
            .on_request(DefaultOnRequest::new().level(Level::INFO))
            .on_response(DefaultOnResponse::new().level(Level::INFO));
        router.layer(http_trace)
    } else {
        router
    }
}

/// Build the gateway base router and `AppState` without throttle, middleware,
/// or state consumption. Callers can merge additional routes (e.g. web-UI)
/// before calling [`finalize_gateway_app`].
#[cfg(feature = "push-notifications")]
fn build_gateway_base_internal(
    state: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
    push_service: Option<Arc<moltis_gateway::push::PushService>>,
    webauthn_registry: Option<SharedWebAuthnRegistry>,
) -> GatewayBase {
    let mut router = Router::new()
        .route("/health", get(health_handler))
        .route("/ws/chat", get(ws_upgrade_handler))
        .route("/ws", get(ws_upgrade_handler));

    // Nest auth routes if credential store is available.
    if let Some(ref cred_store) = state.credential_store {
        let auth_state = AuthState {
            credential_store: Arc::clone(cred_store),
            webauthn_registry: webauthn_registry.clone(),
            gateway_state: Arc::clone(&state),
            login_guard: crate::login_guard::LoginGuard::new(),
        };
        router = router.nest("/api/auth", auth_router().with_state(auth_state));
    }

    #[cfg(feature = "graphql")]
    let graphql_schema = crate::graphql_routes::build_graphql_schema(Arc::clone(&state));
    #[cfg(feature = "ngrok")]
    let ngrok_runtime = Arc::new(tokio::sync::RwLock::new(None));
    #[cfg(feature = "ngrok")]
    let ngrok_controller = Arc::new(NgrokController::new(
        Arc::clone(&state),
        webauthn_registry.clone(),
        Arc::clone(&ngrok_runtime),
    ));

    let app_state = AppState {
        gateway: state,
        methods,
        request_throttle: Arc::new(crate::request_throttle::RequestThrottle::new()),
        webauthn_registry: webauthn_registry.clone(),
        #[cfg(feature = "ngrok")]
        ngrok_controller_owner: None,
        #[cfg(feature = "ngrok")]
        ngrok_controller: Arc::downgrade(&ngrok_controller),
        #[cfg(feature = "ngrok")]
        ngrok_runtime,
        push_service,
        #[cfg(feature = "graphql")]
        graphql_schema,
    };

    // GraphQL routes — auth is handled by the global auth_gate in
    // finalize_gateway_app.
    #[cfg(feature = "graphql")]
    {
        router = router.route(
            "/graphql",
            get(crate::graphql_routes::graphql_get_handler)
                .post(crate::graphql_routes::graphql_handler),
        );
    }

    #[cfg(feature = "ngrok")]
    {
        (router, app_state, ngrok_controller)
    }
    #[cfg(not(feature = "ngrok"))]
    {
        (router, app_state)
    }
}

/// Build the gateway base router and `AppState` without throttle, middleware,
/// or state consumption. Callers can merge additional routes (e.g. web-UI)
/// before calling [`finalize_gateway_app`].
#[cfg(feature = "push-notifications")]
pub fn build_gateway_base(
    state: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
    push_service: Option<Arc<moltis_gateway::push::PushService>>,
    webauthn_registry: Option<SharedWebAuthnRegistry>,
) -> (Router<AppState>, AppState) {
    #[cfg(feature = "ngrok")]
    let (router, mut app_state, ngrok_controller) =
        build_gateway_base_internal(state, methods, push_service, webauthn_registry);
    #[cfg(feature = "ngrok")]
    attach_ngrok_controller_owner(&mut app_state, &ngrok_controller);
    #[cfg(not(feature = "ngrok"))]
    let (router, app_state) =
        build_gateway_base_internal(state, methods, push_service, webauthn_registry);
    (router, app_state)
}

/// Build the gateway base router and `AppState` without throttle, middleware,
/// or state consumption. Callers can merge additional routes (e.g. web-UI)
/// before calling [`finalize_gateway_app`].
#[cfg(not(feature = "push-notifications"))]
fn build_gateway_base_internal(
    state: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
    webauthn_registry: Option<SharedWebAuthnRegistry>,
) -> GatewayBase {
    let mut router = Router::new()
        .route("/health", get(health_handler))
        .route("/ws/chat", get(ws_upgrade_handler))
        .route("/ws", get(ws_upgrade_handler));

    // Add Prometheus metrics endpoint (unauthenticated for scraping).
    #[cfg(feature = "prometheus")]
    {
        router = router.route(
            "/metrics",
            get(crate::metrics_routes::prometheus_metrics_handler),
        );
    }

    // Nest auth routes if credential store is available.
    if let Some(ref cred_store) = state.credential_store {
        let auth_state = AuthState {
            credential_store: Arc::clone(cred_store),
            webauthn_registry: webauthn_registry.clone(),
            gateway_state: Arc::clone(&state),
            login_guard: crate::login_guard::LoginGuard::new(),
        };
        router = router.nest("/api/auth", auth_router().with_state(auth_state));
    }

    #[cfg(feature = "graphql")]
    let graphql_schema = crate::graphql_routes::build_graphql_schema(Arc::clone(&state));
    #[cfg(feature = "ngrok")]
    let ngrok_runtime = Arc::new(tokio::sync::RwLock::new(None));
    #[cfg(feature = "ngrok")]
    let ngrok_controller = Arc::new(NgrokController::new(
        Arc::clone(&state),
        webauthn_registry.clone(),
        Arc::clone(&ngrok_runtime),
    ));

    let app_state = AppState {
        gateway: state,
        methods,
        request_throttle: Arc::new(crate::request_throttle::RequestThrottle::new()),
        webauthn_registry: webauthn_registry.clone(),
        #[cfg(feature = "ngrok")]
        ngrok_controller_owner: None,
        #[cfg(feature = "ngrok")]
        ngrok_controller: Arc::downgrade(&ngrok_controller),
        #[cfg(feature = "ngrok")]
        ngrok_runtime,
        #[cfg(feature = "graphql")]
        graphql_schema,
    };

    // GraphQL routes — auth is handled by the global auth_gate in
    // finalize_gateway_app.
    #[cfg(feature = "graphql")]
    {
        router = router.route(
            "/graphql",
            get(crate::graphql_routes::graphql_get_handler)
                .post(crate::graphql_routes::graphql_handler),
        );
    }

    #[cfg(feature = "ngrok")]
    {
        (router, app_state, ngrok_controller)
    }
    #[cfg(not(feature = "ngrok"))]
    {
        (router, app_state)
    }
}

/// Build the gateway base router and `AppState` without throttle, middleware,
/// or state consumption. Callers can merge additional routes (e.g. web-UI)
/// before calling [`finalize_gateway_app`].
#[cfg(not(feature = "push-notifications"))]
pub fn build_gateway_base(
    state: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
    webauthn_registry: Option<SharedWebAuthnRegistry>,
) -> (Router<AppState>, AppState) {
    #[cfg(feature = "ngrok")]
    let (router, mut app_state, ngrok_controller) =
        build_gateway_base_internal(state, methods, webauthn_registry);
    #[cfg(feature = "ngrok")]
    attach_ngrok_controller_owner(&mut app_state, &ngrok_controller);
    #[cfg(not(feature = "ngrok"))]
    let (router, app_state) = build_gateway_base_internal(state, methods, webauthn_registry);
    (router, app_state)
}

/// Apply throttle, auth gate, middleware, and state to a base router,
/// producing the final `Router` ready for `axum::serve`.
pub fn finalize_gateway_app(
    router: Router<AppState>,
    app_state: AppState,
    http_request_logs: bool,
) -> Router {
    let cors = build_cors_layer();
    // Auth gate covers the entire router — public paths are exempted inside
    // `is_public_path()`.  Only compiled when the web-ui feature is enabled
    // (matches the old architecture where auth_gate was global).
    #[cfg(feature = "web-ui")]
    let router = router.layer(axum::middleware::from_fn_with_state(
        app_state.clone(),
        crate::auth_middleware::auth_gate,
    ));
    // Vault guard blocks API requests when the vault is sealed (not
    // uninitialized). Applied after auth_gate so sealed state is checked
    // only for authenticated requests.
    #[cfg(feature = "vault")]
    let router = router.layer(axum::middleware::from_fn_with_state(
        app_state.clone(),
        crate::auth_middleware::vault_guard,
    ));
    let router = router.layer(axum::middleware::from_fn_with_state(
        app_state.clone(),
        crate::request_throttle::throttle_gate,
    ));
    // HSTS: instruct browsers to always use HTTPS once they've connected securely.
    let router = if app_state.gateway.is_secure() {
        use axum::http::{HeaderValue, header};
        router.layer(SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ))
    } else {
        router
    };
    let router = apply_middleware_stack(router, cors, http_request_logs);
    router.with_state(app_state)
}

/// Convenience wrapper: build base + finalize in one call (used by tests).
#[cfg(feature = "push-notifications")]
pub fn build_gateway_app(
    state: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
    push_service: Option<Arc<moltis_gateway::push::PushService>>,
    http_request_logs: bool,
    webauthn_registry: Option<SharedWebAuthnRegistry>,
) -> Router {
    let (router, app_state) = build_gateway_base(state, methods, push_service, webauthn_registry);
    finalize_gateway_app(router, app_state, http_request_logs)
}

/// Convenience wrapper: build base + finalize in one call (used by tests).
#[cfg(not(feature = "push-notifications"))]
pub fn build_gateway_app(
    state: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
    http_request_logs: bool,
    webauthn_registry: Option<SharedWebAuthnRegistry>,
) -> Router {
    let (router, app_state) = build_gateway_base(state, methods, webauthn_registry);
    finalize_gateway_app(router, app_state, http_request_logs)
}
/// A fully wired gateway (app router + shared state), ready to be served.
///
/// Created by [`prepare_gateway`]. Callers bind their own TCP listener and
/// feed `app` to `axum::serve` (or an equivalent). Background tasks (metrics,
/// MCP health, cron, etc.) are already spawned on the current tokio runtime.
pub struct PreparedGateway {
    /// The composed application router.
    pub app: Router,
    /// Shared gateway state (sessions, services, config, etc.).
    pub state: Arc<GatewayState>,
    /// The port the gateway was configured for.
    pub port: u16,
    /// Metadata collected during setup, used by [`start_gateway`] for the
    /// startup banner. Not relevant for bridge callers.
    pub(crate) banner: BannerMeta,
    /// Network audit buffer for real-time streaming (present when
    /// the `trusted-network` feature is enabled and the proxy is active).
    #[cfg(feature = "trusted-network")]
    pub audit_buffer: Option<moltis_gateway::network_audit::NetworkAuditBuffer>,
    /// Keeps the trusted-network proxy alive for the server's full lifetime.
    /// Dropping this sender closes the watch channel, which is the proxy's
    /// shutdown signal.
    #[cfg(feature = "trusted-network")]
    pub _proxy_shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
}

/// Internal metadata for the startup banner printed by [`start_gateway`].
pub struct BannerMeta {
    pub provider_summary: String,
    pub mcp_configured_count: usize,
    pub method_count: usize,
    pub sandbox_backend_name: String,
    pub data_dir: PathBuf,
    pub openclaw_status: String,
    pub setup_code_display: Option<String>,
    pub webauthn_registry: Option<SharedWebAuthnRegistry>,
    #[cfg(feature = "ngrok")]
    pub ngrok_controller: Arc<NgrokController>,
    pub browser_for_lifecycle: Arc<dyn moltis_gateway::services::BrowserService>,
    pub browser_tool_for_warmup: Option<Arc<dyn moltis_agents::tool_registry::AgentTool>>,
    pub config: moltis_config::schema::MoltisConfig,
    #[cfg(feature = "tailscale")]
    pub tailscale_mode: TailscaleMode,
    #[cfg(feature = "tailscale")]
    pub tailscale_reset_on_exit: bool,
}

/// Prepare the full gateway: load config, run migrations, wire services,
/// spawn background tasks, and return the composed axum application.
///
/// This is the HTTP layer on top of [`prepare_gateway_core`]. The swift-bridge
/// calls this directly and manages its own TCP listener + graceful shutdown.
///
/// `extra_routes` is an optional callback that returns additional routes
/// (e.g. the web-UI) to merge before finalization.
#[allow(clippy::expect_used)]
pub async fn prepare_gateway(
    bind: &str,
    port: u16,
    no_tls: bool,
    log_buffer: Option<moltis_gateway::logs::LogBuffer>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    #[cfg(feature = "tailscale")] tailscale_opts: Option<TailscaleOpts>,
    extra_routes: Option<RouteEnhancer>,
    session_event_bus: Option<SessionEventBus>,
) -> anyhow::Result<PreparedGateway> {
    // Install a process-level rustls CryptoProvider early, before any channel
    // plugin (Slack, Discord, etc.) creates outbound TLS connections via
    // hyper-rustls.  Without this, `--no-tls` deployments skip the TLS cert
    // setup path where `install_default()` previously lived, causing a panic
    // the first time an outbound HTTPS request is made (see #329).
    #[cfg(feature = "tls")]
    let _ = rustls::crypto::ring::default_provider().install_default();

    #[cfg(feature = "tailscale")]
    let tailscale_mode_override = tailscale_opts.as_ref().map(|opts| opts.mode.clone());
    #[cfg(feature = "tailscale")]
    let tailscale_reset_on_exit_override = tailscale_opts.as_ref().map(|opts| opts.reset_on_exit);
    #[cfg(not(feature = "tailscale"))]
    let tailscale_mode_override: Option<String> = None;
    #[cfg(not(feature = "tailscale"))]
    let tailscale_reset_on_exit_override: Option<bool> = None;

    let core = prepare_gateway_core(
        bind,
        port,
        no_tls,
        log_buffer,
        config_dir,
        data_dir,
        tailscale_mode_override,
        tailscale_reset_on_exit_override,
        session_event_bus,
    )
    .await?;

    let PreparedGatewayCore {
        state,
        methods,
        webauthn_registry,
        msteams_webhook_plugin,
        #[cfg(feature = "slack")]
        slack_webhook_plugin,
        #[cfg(feature = "push-notifications")]
        push_service,
        #[cfg(feature = "trusted-network")]
            audit_buffer: audit_buffer_for_broadcast,
        #[cfg(feature = "trusted-network")]
        _proxy_shutdown_tx,
        sandbox_router,
        browser_for_lifecycle,
        browser_tool_for_warmup,
        cron_service,
        log_buffer,
        config,
        data_dir,
        provider_summary,
        mcp_configured_count,
        openclaw_status: openclaw_startup_status,
        setup_code_display,
        port,
        tls_enabled: tls_enabled_for_gateway,
        #[cfg(feature = "tailscale")]
        tailscale_mode,
        #[cfg(feature = "tailscale")]
        tailscale_reset_on_exit,
        ..
    } = core;

    #[cfg(feature = "push-notifications")]
    #[cfg(feature = "ngrok")]
    let (router, mut app_state, ngrok_controller) = build_gateway_base_internal(
        Arc::clone(&state),
        Arc::clone(&methods),
        push_service,
        webauthn_registry.clone(),
    );
    #[cfg(feature = "push-notifications")]
    #[cfg(feature = "ngrok")]
    attach_ngrok_controller_owner(&mut app_state, &ngrok_controller);
    #[cfg(all(feature = "push-notifications", not(feature = "ngrok")))]
    let (router, app_state) = build_gateway_base(
        Arc::clone(&state),
        Arc::clone(&methods),
        push_service,
        webauthn_registry.clone(),
    );
    #[cfg(not(feature = "push-notifications"))]
    #[cfg(feature = "ngrok")]
    let (router, mut app_state, ngrok_controller) = build_gateway_base_internal(
        Arc::clone(&state),
        Arc::clone(&methods),
        webauthn_registry.clone(),
    );
    #[cfg(not(feature = "push-notifications"))]
    #[cfg(feature = "ngrok")]
    attach_ngrok_controller_owner(&mut app_state, &ngrok_controller);
    #[cfg(all(not(feature = "push-notifications"), not(feature = "ngrok")))]
    let (router, app_state) = build_gateway_base(
        Arc::clone(&state),
        Arc::clone(&methods),
        webauthn_registry.clone(),
    );

    // Merge caller-provided routes (e.g. web-UI) before finalization.
    let router = if let Some(enhance) = extra_routes {
        router.merge(enhance())
    } else {
        router
    };

    let mut app = finalize_gateway_app(router, app_state, config.server.http_request_logs);

    {
        let teams_plugin_for_webhook = Arc::clone(&msteams_webhook_plugin);
        let state_for_teams_webhook = Arc::clone(&state);
        app = app.route(
            "/api/channels/msteams/{account_id}/webhook",
            axum::routing::post(
                move |axum::extract::Path(account_id): axum::extract::Path<String>,
                      axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
                      headers: axum::http::HeaderMap,
                      body: axum::body::Bytes| {
                    let teams_plugin = Arc::clone(&teams_plugin_for_webhook);
                    let gw_state = Arc::clone(&state_for_teams_webhook);
                    async move {
                        // JWT pre-validation: if a JWT validator is configured,
                        // the Authorization header is mandatory and must be valid.
                        // A missing header is treated as an auth failure (not skipped).
                        let jwt_validator = {
                            let plugin = teams_plugin.read().await;
                            plugin.jwt_validator(&account_id)
                        };
                        if let Some(validator) = jwt_validator {
                            let header_str = headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if !validator.validate(header_str).await {
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    Json(serde_json::json!({ "ok": false, "error": "invalid JWT" })),
                                )
                                    .into_response();
                            }
                        }

                        // Get the verifier from the plugin.
                        let verifier = {
                            let plugin = teams_plugin.read().await;
                            plugin.channel_webhook_verifier(&account_id)
                        };
                        let Some(verifier) = verifier else {
                            return (
                                StatusCode::NOT_FOUND,
                                Json(serde_json::json!({ "ok": false, "error": "unknown Teams account" })),
                            )
                                .into_response();
                        };

                        // Inject query-param secret as header for the verifier.
                        let mut merged_headers = headers;
                        if let Some(secret) = query.get("secret")
                            && let Ok(val) = secret.parse()
                        {
                            merged_headers.insert("x-moltis-webhook-secret", val);
                        }

                        // Run the middleware pipeline.
                        match moltis_gateway::channel_webhook_middleware::channel_webhook_gate(
                            verifier.as_ref(),
                            &gw_state.channel_webhook_dedup,
                            &gw_state.channel_webhook_rate_limiter,
                            &account_id,
                            &merged_headers,
                            &body,
                        ) {
                            Err(rejection) => {
                                crate::channel_webhook_middleware::rejection_into_response(
                                    rejection,
                                )
                            },
                            Ok((_, moltis_channels::ChannelWebhookDedupeResult::Duplicate)) => (
                                StatusCode::OK,
                                Json(serde_json::json!({ "ok": true, "deduplicated": true })),
                            )
                                .into_response(),
                            Ok((verified, moltis_channels::ChannelWebhookDedupeResult::New)) => {
                                // Parse verified body.
                                let payload: serde_json::Value =
                                    match serde_json::from_slice(&verified.body) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            return (
                                                StatusCode::BAD_REQUEST,
                                                Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
                                            )
                                                .into_response();
                                        },
                                    };

                                // Spawn processing asynchronously and return 202
                                // immediately. This prevents Teams from retrying
                                // when LLM processing takes longer than ~15 seconds.
                                let account_id_owned = account_id.clone();
                                let teams_plugin_for_spawn = Arc::clone(&teams_plugin);
                                tokio::spawn(async move {
                                    let plugin = teams_plugin_for_spawn.read().await;
                                    if let Err(e) = plugin
                                        .ingest_verified_activity(&account_id_owned, payload)
                                        .await
                                    {
                                        tracing::warn!(
                                            account_id = account_id_owned,
                                            "Teams webhook processing failed: {e}"
                                        );
                                    }
                                });

                                (
                                    StatusCode::ACCEPTED,
                                    Json(serde_json::json!({ "ok": true })),
                                )
                                    .into_response()
                            },
                        }
                    }
                },
            ),
        );
    }
    #[cfg(feature = "slack")]
    {
        // Slack Events API webhook — receives event callbacks.
        let slack_events_plugin = Arc::clone(&slack_webhook_plugin);
        let state_for_slack_events = Arc::clone(&state);
        app = app.route(
            "/api/channels/slack/{account_id}/events",
            axum::routing::post(
                move |axum::extract::Path(account_id): axum::extract::Path<String>,
                      headers: axum::http::HeaderMap,
                      body: axum::body::Bytes| {
                    let plugin = Arc::clone(&slack_events_plugin);
                    let gw_state = Arc::clone(&state_for_slack_events);
                    async move {
                        // Get the verifier from the plugin.
                        let verifier = {
                            let p = plugin.read().await;
                            p.channel_webhook_verifier(&account_id)
                        };
                        let Some(verifier) = verifier else {
                            return (
                                StatusCode::NOT_FOUND,
                                Json(serde_json::json!({ "ok": false, "error": "unknown Slack account" })),
                            )
                                .into_response();
                        };

                        // Run the middleware pipeline.
                        match moltis_gateway::channel_webhook_middleware::channel_webhook_gate(
                            verifier.as_ref(),
                            &gw_state.channel_webhook_dedup,
                            &gw_state.channel_webhook_rate_limiter,
                            &account_id,
                            &headers,
                            &body,
                        ) {
                            Err(rejection) => {
                                crate::channel_webhook_middleware::rejection_into_response(
                                    rejection,
                                )
                            },
                            Ok((_, moltis_channels::ChannelWebhookDedupeResult::Duplicate)) => (
                                StatusCode::OK,
                                Json(serde_json::json!({ "ok": true, "deduplicated": true })),
                            )
                                .into_response(),
                            Ok((verified, moltis_channels::ChannelWebhookDedupeResult::New)) => {
                                // Dispatch to Slack plugin with verified body.
                                let result = {
                                    let p = plugin.read().await;
                                    p.ingest_verified_webhook(&account_id, &verified.body)
                                        .await
                                };
                                match result {
                                    Ok(Some(challenge)) => (
                                        StatusCode::OK,
                                        Json(serde_json::json!({ "challenge": challenge })),
                                    )
                                        .into_response(),
                                    Ok(None) => (
                                        StatusCode::OK,
                                        Json(serde_json::json!({ "ok": true })),
                                    )
                                        .into_response(),
                                    Err(e) => {
                                        let msg = e.to_string();
                                        if msg.contains("unknown") {
                                            (
                                                StatusCode::NOT_FOUND,
                                                Json(serde_json::json!({ "ok": false, "error": msg })),
                                            )
                                                .into_response()
                                        } else {
                                            (
                                                StatusCode::BAD_REQUEST,
                                                Json(serde_json::json!({ "ok": false, "error": msg })),
                                            )
                                                .into_response()
                                        }
                                    },
                                }
                            },
                        }
                    }
                },
            ),
        );

        // Slack interaction webhook — receives button click payloads.
        let slack_interact_plugin = Arc::clone(&slack_webhook_plugin);
        let state_for_slack_interact = Arc::clone(&state);
        app = app.route(
            "/api/channels/slack/{account_id}/interactions",
            axum::routing::post(
                move |axum::extract::Path(account_id): axum::extract::Path<String>,
                      headers: axum::http::HeaderMap,
                      body: axum::body::Bytes| {
                    let plugin = Arc::clone(&slack_interact_plugin);
                    let gw_state = Arc::clone(&state_for_slack_interact);
                    async move {
                        // Get the verifier from the plugin.
                        let verifier = {
                            let p = plugin.read().await;
                            p.channel_webhook_verifier(&account_id)
                        };
                        let Some(verifier) = verifier else {
                            return (
                                StatusCode::NOT_FOUND,
                                Json(serde_json::json!({ "ok": false, "error": "unknown Slack account" })),
                            )
                                .into_response();
                        };

                        // Run the middleware pipeline.
                        match moltis_gateway::channel_webhook_middleware::channel_webhook_gate(
                            verifier.as_ref(),
                            &gw_state.channel_webhook_dedup,
                            &gw_state.channel_webhook_rate_limiter,
                            &account_id,
                            &headers,
                            &body,
                        ) {
                            Err(rejection) => {
                                crate::channel_webhook_middleware::rejection_into_response(
                                    rejection,
                                )
                            },
                            Ok((_, moltis_channels::ChannelWebhookDedupeResult::Duplicate)) => (
                                StatusCode::OK,
                                Json(serde_json::json!({ "ok": true, "deduplicated": true })),
                            )
                                .into_response(),
                            Ok((verified, moltis_channels::ChannelWebhookDedupeResult::New)) => {
                                // Dispatch to Slack plugin with verified body.
                                let result = {
                                    let p = plugin.read().await;
                                    p.ingest_verified_interaction_webhook(
                                        &account_id,
                                        &verified.body,
                                    )
                                    .await
                                };
                                match result {
                                    Ok(()) => (
                                        StatusCode::OK,
                                        Json(serde_json::json!({ "ok": true })),
                                    )
                                        .into_response(),
                                    Err(e) => (
                                        StatusCode::BAD_REQUEST,
                                        Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
                                    )
                                        .into_response(),
                                }
                            },
                        }
                    }
                },
            ),
        );
    }

    // ── Generic webhook ingress ────────────────────────────────────────────
    {
        fn webhook_cors_headers(mut resp: axum::response::Response) -> axum::response::Response {
            use axum::http::HeaderValue;
            let h = resp.headers_mut();
            h.insert(
                axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
            h.insert(
                axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("POST, OPTIONS"),
            );
            h.insert(axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("Content-Type, Authorization, X-Hub-Signature-256, X-GitHub-Event, X-GitHub-Delivery, X-Gitlab-Token, X-Gitlab-Event, Stripe-Signature, X-Webhook-Secret, X-Event-Type, X-Delivery-Id, Idempotency-Key, Linear-Signature, X-PagerDuty-Signature, Sentry-Hook-Signature"));
            h.insert(
                axum::http::header::ACCESS_CONTROL_MAX_AGE,
                HeaderValue::from_static("86400"),
            );
            resp
        }

        // OPTIONS preflight handler.
        app = app.route(
            "/api/webhooks/ingest/{public_id}",
            axum::routing::options(move |_: axum::extract::Path<String>| async move {
                webhook_cors_headers(StatusCode::NO_CONTENT.into_response())
            }),
        );

        let state_for_webhook_ingest = Arc::clone(&state);
        app = app.route(
            "/api/webhooks/ingest/{public_id}",
            axum::routing::post(
                move |axum::extract::Path(public_id): axum::extract::Path<String>,
                      ConnectInfo(peer): ConnectInfo<SocketAddr>,
                      headers: axum::http::HeaderMap,
                      body: axum::body::Bytes| {
                    let gw = Arc::clone(&state_for_webhook_ingest);
                    async move {
                        // Extract remote IP. Behind a proxy, trust forwarded
                        // headers; otherwise use the real TCP peer address.
                        let remote_ip = if gw.behind_proxy {
                            headers
                                .get("x-forwarded-for")
                                .and_then(|v| v.to_str().ok())
                                .and_then(|v| v.split(',').next())
                                .map(|s| s.trim().to_string())
                                .or_else(|| {
                                    headers
                                        .get("x-real-ip")
                                        .and_then(|v| v.to_str().ok())
                                        .map(|s| s.trim().to_string())
                                })
                                .or_else(|| Some(peer.ip().to_string()))
                        } else {
                            Some(peer.ip().to_string())
                        };

                        let resp = async {
                            let Some(store) = gw.webhook_store.get() else {
                                return (
                                    StatusCode::NOT_FOUND,
                                    Json(serde_json::json!({ "error": "webhooks not configured" })),
                                )
                                    .into_response();
                            };

                            // Look up webhook by public_id.
                            let webhook = match store.get_webhook_by_public_id(&public_id).await {
                                Ok(w) if w.enabled => w,
                                Ok(_) => {
                                    return (
                                        StatusCode::NOT_FOUND,
                                        Json(serde_json::json!({ "error": "webhook not found" })),
                                    )
                                        .into_response();
                                },
                                Err(_) => {
                                    return (
                                        StatusCode::NOT_FOUND,
                                        Json(serde_json::json!({ "error": "webhook not found" })),
                                    )
                                        .into_response();
                                },
                            };

                            #[allow(unused_mut)]
                            // Secret decryption mutates the webhook only when the vault feature is enabled.
                            let mut webhook = webhook;

                            #[cfg(feature = "vault")]
                            if let Err(error) = moltis_gateway::webhooks::decrypt_webhook_secrets(
                                &mut webhook,
                                gw.vault.as_ref(),
                            )
                            .await
                            {
                                tracing::warn!(
                                    public_id = %webhook.public_id,
                                    error = %error,
                                    "webhook secrets unavailable for runtime verification"
                                );
                                return (
                                    StatusCode::SERVICE_UNAVAILABLE,
                                    Json(serde_json::json!({
                                        "error": "webhook secrets unavailable",
                                    })),
                                )
                                    .into_response();
                            }

                            // Check CIDR allowlist (before auth to avoid timing side-channels).
                            if !webhook.allowed_cidrs.is_empty() {
                                let allowed = match &remote_ip {
                                    Some(ip) => {
                                        if let Ok(addr) = ip.parse::<std::net::IpAddr>() {
                                            webhook.allowed_cidrs.iter().any(|cidr| {
                                                cidr.parse::<ipnet::IpNet>()
                                                    .map(|net| net.contains(&addr))
                                                    .unwrap_or_else(|_| {
                                                        // Fall back to exact string match.
                                                        cidr == ip
                                                    })
                                            })
                                        } else {
                                            // IP couldn't be parsed — no match.
                                            false
                                        }
                                    },
                                    None => false, // No IP available — can't match allowlist.
                                };
                                if !allowed {
                                    return (
                                        StatusCode::FORBIDDEN,
                                        Json(serde_json::json!({ "error": "IP not in allowlist" })),
                                    )
                                        .into_response();
                                }
                            }

                            // Check body size limit.
                            if body.len() > webhook.max_body_bytes {
                                return (
                                    StatusCode::PAYLOAD_TOO_LARGE,
                                    Json(serde_json::json!({
                                        "error": "payload too large",
                                        "maxBytes": webhook.max_body_bytes,
                                    })),
                                )
                                    .into_response();
                            }

                            // Verify authentication.
                            if let Err(e) = moltis_webhooks::auth::verify(
                                &webhook.auth_mode,
                                webhook.auth_config.as_ref(),
                                &headers,
                                &body,
                            ) {
                                tracing::warn!(
                                    webhook_id = webhook.id,
                                    public_id = %webhook.public_id,
                                    error = %e,
                                    "webhook auth verification failed"
                                );
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    Json(serde_json::json!({ "error": "authentication failed" })),
                                )
                                    .into_response();
                            }

                            // Parse event type and delivery key from source profile.
                            let profile_registry =
                                moltis_webhooks::profiles::ProfileRegistry::new();
                            let profile = profile_registry.get(&webhook.source_profile);
                            let event_type =
                                profile.and_then(|p| p.parse_event_type(&headers, &body));
                            let delivery_key =
                                profile.and_then(|p| p.parse_delivery_key(&headers, &body));

                            // Check event filter.
                            if let Some(ref et) = event_type
                                && !webhook.event_filter.accepts(et)
                            {
                                return (
                                    StatusCode::OK,
                                    Json(serde_json::json!({
                                        "status": "filtered",
                                        "eventType": et,
                                    })),
                                )
                                    .into_response();
                            }

                            // Check rate limit.
                            if !gw
                                .webhook_rate_limiter
                                .check(webhook.id, webhook.rate_limit_per_minute)
                            {
                                return (
                                    StatusCode::TOO_MANY_REQUESTS,
                                    Json(serde_json::json!({ "error": "rate limited" })),
                                )
                                    .into_response();
                            }

                            // Dedup check.
                            if let Some(ref dk) = delivery_key {
                                match moltis_webhooks::dedup::check_duplicate(
                                    store.as_ref(),
                                    webhook.id,
                                    Some(dk.as_str()),
                                )
                                .await
                                {
                                    Ok(Some(existing_id)) => {
                                        return (
                                            StatusCode::OK,
                                            Json(serde_json::json!({
                                                "status": "deduplicated",
                                                "existingDeliveryId": existing_id,
                                            })),
                                        )
                                            .into_response();
                                    },
                                    Ok(None) => { /* new delivery, continue */ },
                                    Err(e) => {
                                        tracing::error!(
                                            webhook_id = webhook.id,
                                            error = %e,
                                            "dedup check failed"
                                        );
                                        // Continue despite dedup error — better to
                                        // accept a potential duplicate than reject.
                                    },
                                }
                            }

                            // Build timestamp.
                            let received_at = time::OffsetDateTime::now_utc()
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());

                            // Extract entity key.
                            let entity_key = if let (Some(p), Some(et)) = (profile, &event_type) {
                                let body_val: serde_json::Value =
                                    serde_json::from_slice(&body).unwrap_or_default();
                                p.entity_key(et, &body_val)
                            } else {
                                None
                            };

                            // Extract safe headers for audit logging.
                            let safe_headers =
                                moltis_webhooks::normalize::extract_safe_headers(&headers);
                            let headers_json = serde_json::to_string(&safe_headers).ok();

                            let content_type = headers
                                .get("content-type")
                                .and_then(|v| v.to_str().ok())
                                .map(String::from);

                            // Persist delivery.
                            let delivery = moltis_webhooks::store::NewDelivery {
                                webhook_id: webhook.id,
                                received_at: received_at.clone(),
                                status: moltis_webhooks::types::DeliveryStatus::Queued,
                                event_type: event_type.clone(),
                                entity_key,
                                delivery_key,
                                http_method: Some("POST".into()),
                                content_type,
                                remote_ip: remote_ip.clone(),
                                headers_json,
                                body_size: body.len(),
                                body_blob: Some(body.to_vec()),
                                rejection_reason: None,
                            };

                            let delivery_id = match store.insert_delivery(&delivery).await {
                                Ok(id) => id,
                                Err(e) => {
                                    tracing::error!(
                                        webhook_id = webhook.id,
                                        error = %e,
                                        "failed to persist webhook delivery"
                                    );
                                    return (
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        Json(serde_json::json!({
                                            "error": "failed to persist delivery"
                                        })),
                                    )
                                        .into_response();
                                },
                            };

                            // Update denormalized delivery count.
                            if let Err(e) = store
                                .increment_delivery_count(webhook.id, &received_at)
                                .await
                            {
                                tracing::warn!(
                                    webhook_id = webhook.id,
                                    error = %e,
                                    "failed to increment delivery count"
                                );
                            }

                            // Queue for async processing.
                            if let Some(tx) = gw.webhook_worker_tx.get()
                                && let Err(e) = tx.send(delivery_id).await
                            {
                                tracing::error!(
                                    delivery_id,
                                    error = %e,
                                    "failed to queue webhook delivery for processing"
                                );
                            }

                            (
                                StatusCode::ACCEPTED,
                                Json(serde_json::json!({
                                    "deliveryId": delivery_id,
                                    "status": "queued",
                                    "webhookId": webhook.public_id,
                                    "eventType": event_type,
                                    "receivedAt": received_at,
                                })),
                            )
                                .into_response()
                        }
                        .await;
                        webhook_cors_headers(resp)
                    }
                },
            ),
        );
    }

    let method_count = methods.method_names().len();

    finalize_prepared_gateway(FinalizeGatewayArgs {
        bind,
        port,
        tls_enabled_for_gateway,
        state,
        browser_for_lifecycle,
        browser_tool_for_warmup,
        sandbox_router,
        cron_service,
        log_buffer,
        config,
        data_dir,
        provider_summary,
        mcp_configured_count,
        method_count,
        openclaw_startup_status,
        setup_code_display,
        webauthn_registry,
        #[cfg(feature = "ngrok")]
        ngrok_controller,
        #[cfg(feature = "trusted-network")]
        audit_buffer_for_broadcast,
        #[cfg(feature = "trusted-network")]
        _proxy_shutdown_tx,
        #[cfg(feature = "tailscale")]
        tailscale_mode,
        #[cfg(feature = "tailscale")]
        tailscale_reset_on_exit,
        app,
    })
    .await
}
