# Feature: Session Feedback

## Overview

Add an in-session feedback mechanism. The gateway periodically sends a
`feedback.requested` WebSocket event to connected clients. Clients respond
with an RPC call containing a 0–4 rating. The gateway forwards the rating
(plus session ID) to a configurable HTTP endpoint.

## Design

### New event: `feedback.requested`

Broadcast to all operator clients on a timer, similar to the existing tick
system. Payload: `{ "prompt": "How is this session going? (0-4)" }`.

**Triggering logic** (configurable defaults):
- Minimum **10 minutes** into a session before first prompt
- Minimum **30 minutes** between subsequent prompts
- Only sent when there are active operator clients

### New RPC method: `feedback.submit`

Client sends: `{ "rating": 2 }` (integer 0–4).
Gateway validates the rating, then POSTs to the configured endpoint:

```json
{ "rating": 2, "session_id": "..." }
```

Returns `{ "ok": true }` to the client (so the client can show a
confirmation).

### Config addition (`moltis.toml`)

```toml
[feedback]
enabled = true
endpoint = "https://example.com/feedback"
```

Add `FeedbackConfig` to `MoltisConfig` in `crates/config/src/schema.rs`.

### Files to modify/create

| File | Change |
|---|---|
| `crates/config/src/schema.rs` | Add `FeedbackConfig` struct and field on `MoltisConfig` |
| `crates/protocol/src/lib.rs` | Add `feedback.requested` event name constant |
| `crates/gateway/src/broadcast.rs` | Add `broadcast_feedback_request()` helper |
| `crates/gateway/src/server.rs` | Spawn feedback timer task (like tick timer) |
| `crates/gateway/src/methods.rs` | Add `feedback.submit` RPC handler |
| `crates/gateway/src/feedback.rs` | **New file** — feedback timer logic + HTTP POST to endpoint |

### Feedback timer (`feedback.rs`)

```rust
pub async fn feedback_loop(state: Arc<GatewayState>, config: FeedbackConfig) {
    // Wait initial delay (10 min default)
    // Loop: wait interval (30 min default), then broadcast feedback.requested
}
```

Uses `reqwest::Client` (already a dependency) to POST ratings to the
configured endpoint.

## Verification

1. `cargo check` — compiles
2. `cargo +nightly clippy` — no warnings
3. `cargo test` — unit tests for:
   - Config deserialization with/without feedback section
   - Rating validation (reject outside 0–4)
   - HTTP POST payload construction (mock the endpoint)
   - Timer logic (initial delay, interval)
4. Manual: start gateway with `[feedback]` config, connect a WebSocket client,
   wait for the event, send `feedback.submit`, verify the POST hits the endpoint
