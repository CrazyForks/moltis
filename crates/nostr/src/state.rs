//! Per-account runtime state for Nostr.

use {
    moltis_channels::otp::OtpState,
    nostr_sdk::prelude::{Client, Keys, ToBech32},
    tokio_util::sync::CancellationToken,
};

use crate::config::NostrAccountConfig;

/// Runtime state for a single active Nostr account.
pub struct AccountState {
    /// The nostr-sdk client connected to relays.
    pub client: Client,
    /// Bot key pair (secret + public).
    pub keys: Keys,
    /// Parsed account configuration.
    pub config: NostrAccountConfig,
    /// Cancellation token for the subscription loop.
    pub cancel: CancellationToken,
    /// OTP self-approval state for non-allowlisted senders.
    pub otp: OtpState,
}

impl std::fmt::Debug for AccountState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pk = self
            .keys
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| self.keys.public_key().to_hex());
        f.debug_struct("AccountState")
            .field("pubkey", &pk)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}
