use std::sync::Arc;

use axum::{
    extract::State,
    extract::WebSocketUpgrade,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use moltis_protocol::TICK_INTERVAL_MS;

use crate::broadcast::broadcast_tick;
use crate::methods::MethodRegistry;
use crate::state::GatewayState;
use crate::ws::handle_connection;

// ── Shared app state ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    gateway: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
}

// ── Server startup ───────────────────────────────────────────────────────────

/// Start the gateway HTTP + WebSocket server.
pub async fn start_gateway(bind: &str, port: u16) -> anyhow::Result<()> {
    let state = GatewayState::new();
    let methods = Arc::new(MethodRegistry::new());

    let app_state = AppState {
        gateway: Arc::clone(&state),
        methods: Arc::clone(&methods),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/ws", get(ws_upgrade_handler))
        .route("/", get(root_handler))
        .layer(cors)
        .with_state(app_state);

    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // Startup banner.
    info!("┌─────────────────────────────────────────────┐");
    info!("│  moltis gateway v{}                     │", state.version);
    info!("│  protocol v{}, listening on {}  │", moltis_protocol::PROTOCOL_VERSION, addr);
    info!("│  {} methods registered                      │", methods.method_names().len());
    info!("└─────────────────────────────────────────────┘");

    // Spawn tick timer.
    let tick_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(TICK_INTERVAL_MS));
        loop {
            interval.tick().await;
            broadcast_tick(&tick_state).await;
        }
    });

    // Run the server.
    axum::serve(listener, app).await?;
    Ok(())
}

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let count = state.gateway.client_count().await;
    Json(serde_json::json!({
        "status": "ok",
        "version": state.gateway.version,
        "protocol": moltis_protocol::PROTOCOL_VERSION,
        "connections": count,
    }))
}

async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        handle_connection(socket, state.gateway, state.methods)
    })
}

async fn root_handler() -> impl IntoResponse {
    Html(
        r#"<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>moltis gateway</title>
  <style>
    body { font-family: system-ui, sans-serif; background: #0a0a0a; color: #e0e0e0;
           display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; }
    .container { text-align: center; }
    h1 { font-size: 2rem; font-weight: 300; letter-spacing: 0.05em; }
    p { color: #888; font-size: 0.9rem; }
    code { background: #1a1a1a; padding: 2px 8px; border-radius: 4px; font-size: 0.85rem; }
  </style>
</head>
<body>
  <div class="container">
    <h1>moltis</h1>
    <p>Gateway is running. Connect via WebSocket at <code>/ws</code></p>
  </div>
</body>
</html>"#,
    )
}
