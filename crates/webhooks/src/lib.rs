//! Generic webhook ingress for Moltis.
//!
//! Provides named inbound HTTP endpoints that trigger agent sessions.
//! Each delivery is verified, deduplicated, persisted, and processed
//! asynchronously via the existing chat/session infrastructure.

pub mod auth;
pub mod dedup;
pub mod error;
pub mod filter;
pub mod normalize;
pub mod profiles;
pub mod rate_limit;
pub mod store;
pub mod types;
pub mod worker;

pub use error::{Error, Result};

/// Run database migrations for the webhooks crate.
///
/// Creates the `webhooks`, `webhook_deliveries`, and `webhook_response_actions`
/// tables. Call at application startup.
pub async fn run_migrations(pool: &sqlx::SqlitePool) -> Result<()> {
    // Enable foreign key enforcement so ON DELETE CASCADE works.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(pool)
        .await
        .map_err(Error::from)?;
    sqlx::migrate!("./migrations")
        .set_ignore_missing(true)
        .run(pool)
        .await?;
    Ok(())
}
