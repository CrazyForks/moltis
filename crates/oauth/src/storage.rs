use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::config_dir::moltis_config_dir;
use crate::types::OAuthTokens;

/// File-based token storage at `~/.config/moltis/oauth_tokens.json`.
#[derive(Debug, Clone)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    pub fn new() -> Self {
        let path = moltis_config_dir().join("oauth_tokens.json");
        Self { path }
    }

    /// Create a token store at a specific path (useful for testing).
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self, provider: &str) -> Option<OAuthTokens> {
        let data = std::fs::read_to_string(&self.path).ok()?;
        let map: HashMap<String, OAuthTokens> = serde_json::from_str(&data).ok()?;
        map.get(provider).cloned()
    }

    pub fn save(&self, provider: &str, tokens: &OAuthTokens) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut map: HashMap<String, OAuthTokens> = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|d| serde_json::from_str(&d).ok())
            .unwrap_or_default();

        map.insert(provider.to_string(), tokens.clone());

        let data = serde_json::to_string_pretty(&map)?;
        std::fs::write(&self.path, &data)?;

        // Set file permissions to 0600 on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }

    pub fn delete(&self, provider: &str) -> Result<()> {
        let data = match std::fs::read_to_string(&self.path) {
            Ok(d) => d,
            Err(_) => return Ok(()),
        };

        let mut map: HashMap<String, OAuthTokens> = serde_json::from_str(&data)?;
        map.remove(provider);

        let data = serde_json::to_string_pretty(&map)?;
        std::fs::write(&self.path, &data)?;
        Ok(())
    }

    pub fn list(&self) -> Vec<String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|d| serde_json::from_str::<HashMap<String, OAuthTokens>>(&d).ok())
            .map(|m| m.into_keys().collect())
            .unwrap_or_default()
    }
}

impl Default for TokenStore {
    fn default() -> Self {
        Self::new()
    }
}
