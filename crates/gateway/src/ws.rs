use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures::stream::StreamExt;
use futures::SinkExt;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use moltis_protocol::{
    error_codes, ConnectParams, ErrorShape, EventFrame, GatewayFrame, HelloOk,
    Policy, ResponseFrame, ServerInfo, Features, HANDSHAKE_TIMEOUT_MS, PROTOCOL_VERSION,
};

use crate::methods::{MethodContext, MethodRegistry};
use crate::state::{ConnectedClient, GatewayState};

/// Handle a single WebSocket connection through its full lifecycle:
/// handshake → message loop → cleanup.
pub async fn handle_connection(
    socket: WebSocket,
    state: Arc<GatewayState>,
    methods: Arc<MethodRegistry>,
) {
    let conn_id = uuid::Uuid::new_v4().to_string();
    info!(conn_id = %conn_id, "ws: new connection");

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (client_tx, mut client_rx) = mpsc::unbounded_channel::<String>();

    // Spawn write loop: forwards frames from the client_tx channel to the WebSocket.
    let write_conn_id = conn_id.clone();
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = client_rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                debug!(conn_id = %write_conn_id, "ws: write loop closed");
                break;
            }
        }
    });

    // ── Handshake phase ──────────────────────────────────────────────────

    // Wait for the first message (must be a `connect` request).
    let connect_params = match tokio::time::timeout(
        std::time::Duration::from_millis(HANDSHAKE_TIMEOUT_MS),
        wait_for_connect(&mut ws_rx),
    )
    .await
    {
        Ok(Ok((request_id, params))) => {
            // Validate protocol version.
            if params.min_protocol > PROTOCOL_VERSION || params.max_protocol < PROTOCOL_VERSION {
                let err = ResponseFrame::err(
                    &request_id,
                    ErrorShape::new(
                        error_codes::INVALID_REQUEST,
                        format!(
                            "protocol mismatch: server={}, client={}-{}",
                            PROTOCOL_VERSION, params.min_protocol, params.max_protocol
                        ),
                    ),
                );
                let _ = client_tx.send(serde_json::to_string(&err).unwrap());
                drop(client_tx);
                write_handle.abort();
                return;
            }

            // Build and send HelloOk.
            let hello = HelloOk {
                r#type: "hello-ok".into(),
                protocol: PROTOCOL_VERSION,
                server: ServerInfo {
                    version: state.version.clone(),
                    commit: None,
                    host: Some(state.hostname.clone()),
                    conn_id: conn_id.clone(),
                },
                features: Features {
                    methods: methods.method_names(),
                    events: vec![
                        "tick".into(),
                        "shutdown".into(),
                        "agent".into(),
                        "chat".into(),
                        "presence".into(),
                        "health".into(),
                        "exec.approval.requested".into(),
                        "exec.approval.resolved".into(),
                        "device.pair.requested".into(),
                        "device.pair.resolved".into(),
                        "node.pair.requested".into(),
                        "node.pair.resolved".into(),
                        "node.invoke.request".into(),
                    ],
                },
                snapshot: serde_json::json!({}),
                canvas_host_url: None,
                auth: None,
                policy: Policy::default_policy(),
            };
            let resp = ResponseFrame::ok(&request_id, serde_json::to_value(&hello).unwrap());
            let _ = client_tx.send(serde_json::to_string(&resp).unwrap());

            info!(
                conn_id = %conn_id,
                client_id = %params.client.id,
                client_version = %params.client.version,
                role = params.role.as_deref().unwrap_or("operator"),
                "ws: handshake complete"
            );
            params
        }
        Ok(Err(e)) => {
            warn!(conn_id = %conn_id, error = %e, "ws: handshake failed");
            drop(client_tx);
            write_handle.abort();
            return;
        }
        Err(_) => {
            warn!(conn_id = %conn_id, "ws: handshake timeout");
            drop(client_tx);
            write_handle.abort();
            return;
        }
    };

    // Register the client.
    let client = ConnectedClient {
        conn_id: conn_id.clone(),
        connect_params,
        sender: client_tx.clone(),
        connected_at: std::time::Instant::now(),
    };
    let role = client.role().to_string();
    let scopes: Vec<String> = client.scopes().iter().map(|s| s.to_string()).collect();
    state.register_client(client).await;

    // ── Message loop ─────────────────────────────────────────────────────

    while let Some(msg) = ws_rx.next().await {
        let msg = match msg {
            Ok(Message::Text(t)) => t.to_string(),
            Ok(Message::Close(_)) => break,
            Ok(_) => continue, // ignore binary/ping/pong
            Err(e) => {
                debug!(conn_id = %conn_id, error = %e, "ws: read error");
                break;
            }
        };

        let frame: GatewayFrame = match serde_json::from_str(&msg) {
            Ok(f) => f,
            Err(e) => {
                warn!(conn_id = %conn_id, error = %e, "ws: invalid frame");
                let err = EventFrame::new(
                    "error",
                    serde_json::json!({ "message": "invalid frame" }),
                    state.next_seq(),
                );
                let _ = client_tx.send(serde_json::to_string(&err).unwrap());
                continue;
            }
        };

        match frame {
            GatewayFrame::Request(req) => {
                let ctx = MethodContext {
                    request_id: req.id.clone(),
                    method: req.method.clone(),
                    params: req.params.unwrap_or(serde_json::Value::Null),
                    client_conn_id: conn_id.clone(),
                    client_role: role.clone(),
                    client_scopes: scopes.clone(),
                    state: Arc::clone(&state),
                };
                let response = methods.dispatch(ctx).await;
                let _ = client_tx.send(serde_json::to_string(&response).unwrap());
            }
            _ => {
                // Clients should only send requests after handshake.
                debug!(conn_id = %conn_id, "ws: ignoring non-request frame");
            }
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────────────

    let duration = state
        .remove_client(&conn_id)
        .await
        .map(|c| c.connected_at.elapsed())
        .unwrap_or_default();

    info!(
        conn_id = %conn_id,
        duration_secs = duration.as_secs(),
        "ws: connection closed"
    );

    drop(client_tx);
    write_handle.abort();
}

/// Wait for the first `connect` request frame. Returns the request ID and
/// parsed ConnectParams.
async fn wait_for_connect(
    rx: &mut futures::stream::SplitStream<WebSocket>,
) -> anyhow::Result<(String, ConnectParams)> {
    while let Some(msg) = rx.next().await {
        let text = match msg? {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => anyhow::bail!("connection closed before handshake"),
            _ => continue,
        };

        let frame: GatewayFrame = serde_json::from_str(&text)?;
        match frame {
            GatewayFrame::Request(req) => {
                if req.method != "connect" {
                    anyhow::bail!("first message must be 'connect', got '{}'", req.method);
                }
                let params: ConnectParams =
                    serde_json::from_value(req.params.unwrap_or(serde_json::Value::Null))?;
                return Ok((req.id, params));
            }
            _ => anyhow::bail!("first message must be a request frame"),
        }
    }
    anyhow::bail!("connection closed before handshake")
}
