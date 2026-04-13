//! Provider setup service entrypoint.
//!
//! The runtime implementation lives in `service/implementation.rs`; this
//! façade keeps the crate root small and exposes only the public API.

#[path = "service/implementation.rs"]
mod implementation;

pub use self::implementation::{ErrorParser, LiveProviderSetupService};
