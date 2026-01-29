use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, RwLock};

use moltis_protocol::ConnectParams;

// ── Connected client ─────────────────────────────────────────────────────────

/// A WebSocket client currently connected to the gateway.
#[derive(Debug)]
pub struct ConnectedClient {
    pub conn_id: String,
    pub connect_params: ConnectParams,
    /// Channel for sending serialized frames to this client's write loop.
    pub sender: mpsc::UnboundedSender<String>,
    pub connected_at: Instant,
}

impl ConnectedClient {
    pub fn role(&self) -> &str {
        self.connect_params.role.as_deref().unwrap_or("operator")
    }

    pub fn scopes(&self) -> Vec<&str> {
        self.connect_params
            .scopes
            .as_ref()
            .map(|s| s.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes().iter().any(|s| *s == moltis_protocol::scopes::ADMIN || *s == scope)
    }

    /// Send a serialized JSON frame to this client.
    pub fn send(&self, frame: &str) -> bool {
        self.sender.send(frame.to_string()).is_ok()
    }
}

// ── Dedupe cache ─────────────────────────────────────────────────────────────

/// Idempotency cache entry.
struct DedupeEntry {
    _inserted_at: Instant,
}

/// Simple TTL-based idempotency cache.
pub struct DedupeCache {
    entries: HashMap<String, DedupeEntry>,
    ttl: std::time::Duration,
    max_entries: usize,
}

impl Default for DedupeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DedupeCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            ttl: std::time::Duration::from_millis(moltis_protocol::DEDUPE_TTL_MS),
            max_entries: moltis_protocol::DEDUPE_MAX_ENTRIES,
        }
    }

    /// Returns true if the key is a duplicate (already seen within TTL).
    pub fn check_and_insert(&mut self, key: &str) -> bool {
        self.evict_expired();
        if self.entries.contains_key(key) {
            return true;
        }
        if self.entries.len() >= self.max_entries {
            // Evict oldest entry.
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, v)| v._inserted_at)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert(
            key.to_string(),
            DedupeEntry {
                _inserted_at: Instant::now(),
            },
        );
        false
    }

    fn evict_expired(&mut self) {
        let cutoff = Instant::now() - self.ttl;
        self.entries.retain(|_, v| v._inserted_at > cutoff);
    }
}

// ── Gateway state ────────────────────────────────────────────────────────────

/// Shared gateway runtime state, wrapped in Arc for use across async tasks.
pub struct GatewayState {
    /// All connected WebSocket clients, keyed by conn_id.
    pub clients: RwLock<HashMap<String, ConnectedClient>>,
    /// Monotonically increasing sequence counter for broadcast events.
    pub seq: AtomicU64,
    /// Idempotency cache.
    pub dedupe: RwLock<DedupeCache>,
    /// Server version string.
    pub version: String,
    /// Hostname for HelloOk.
    pub hostname: String,
}

impl GatewayState {
    pub fn new() -> Arc<Self> {
        let hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".into());

        Arc::new(Self {
            clients: RwLock::new(HashMap::new()),
            seq: AtomicU64::new(0),
            dedupe: RwLock::new(DedupeCache::new()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            hostname,
        })
    }

    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Register a new client connection.
    pub async fn register_client(&self, client: ConnectedClient) {
        let conn_id = client.conn_id.clone();
        self.clients.write().await.insert(conn_id, client);
    }

    /// Remove a client by conn_id. Returns the removed client if found.
    pub async fn remove_client(&self, conn_id: &str) -> Option<ConnectedClient> {
        self.clients.write().await.remove(conn_id)
    }

    /// Number of connected clients.
    pub async fn client_count(&self) -> usize {
        self.clients.read().await.len()
    }
}
