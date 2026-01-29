/// Config schema types (agents, channels, tools, session, gateway, plugins).
/// Corresponds to src/config/types.ts and zod-schema.*.ts in the TS codebase.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Root configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MoltisConfig {
    pub providers: ProvidersConfig,
    // Future sections:
    // pub agents: AgentsConfig,
    // pub channels: ChannelsConfig,
    // pub gateway: GatewayConfig,
    // pub plugins: PluginsConfig,
}

/// LLM provider configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    /// Provider-specific settings keyed by provider name.
    /// Known keys: "anthropic", "openai", "gemini", "groq", "xai", "deepseek"
    #[serde(flatten)]
    pub providers: HashMap<String, ProviderEntry>,
}

/// Configuration for a single LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderEntry {
    /// Whether this provider is enabled. Defaults to true.
    pub enabled: bool,

    /// Override the API key (optional; env var still takes precedence if set).
    pub api_key: Option<String>,

    /// Override the base URL.
    pub base_url: Option<String>,

    /// Default model ID for this provider.
    pub model: Option<String>,
}

impl Default for ProviderEntry {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: None,
            base_url: None,
            model: None,
        }
    }
}

impl ProvidersConfig {
    /// Check if a provider is enabled (defaults to true if not configured).
    pub fn is_enabled(&self, name: &str) -> bool {
        self.providers
            .get(name)
            .map_or(true, |e| e.enabled)
    }

    /// Get the configured entry for a provider, if any.
    pub fn get(&self, name: &str) -> Option<&ProviderEntry> {
        self.providers.get(name)
    }
}
