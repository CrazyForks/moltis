use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::env_subst::substitute_env;
use crate::schema::MoltisConfig;

/// Standard config file names, checked in order.
const CONFIG_FILENAMES: &[&str] = &[
    "moltis.toml",
    "moltis.yaml",
    "moltis.yml",
    "moltis.json",
];

/// Load config from the given path (any supported format).
pub fn load_config(path: &Path) -> anyhow::Result<MoltisConfig> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let raw = substitute_env(&raw);
    parse_config(&raw, path)
}

/// Load and parse the config file with env substitution and includes.
pub fn load_config_value(path: &Path) -> anyhow::Result<serde_json::Value> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let raw = substitute_env(&raw);
    parse_config_value(&raw, path)
}

/// Discover and load config from standard locations.
///
/// Search order:
/// 1. `./moltis.{toml,yaml,yml,json}` (project-local)
/// 2. `~/.config/moltis/moltis.{toml,yaml,yml,json}` (user-global)
///
/// Returns `MoltisConfig::default()` if no config file is found.
pub fn discover_and_load() -> MoltisConfig {
    if let Some(path) = find_config_file() {
        debug!(path = %path.display(), "loading config");
        match load_config(&path) {
            Ok(cfg) => return cfg,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to load config, using defaults");
            }
        }
    } else {
        debug!("no config file found, using defaults");
    }
    MoltisConfig::default()
}

/// Find the first config file in standard locations.
fn find_config_file() -> Option<PathBuf> {
    // Project-local
    for name in CONFIG_FILENAMES {
        let p = PathBuf::from(name);
        if p.exists() {
            return Some(p);
        }
    }

    // User-global: ~/.config/moltis/
    if let Some(dirs) = directories::ProjectDirs::from("", "", "moltis") {
        let config_dir = dirs.config_dir();
        for name in CONFIG_FILENAMES {
            let p = config_dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }

    None
}

fn parse_config(raw: &str, path: &Path) -> anyhow::Result<MoltisConfig> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("toml");

    match ext {
        "toml" => Ok(toml::from_str(raw)?),
        "yaml" | "yml" => Ok(serde_yaml::from_str(raw)?),
        "json" => Ok(serde_json::from_str(raw)?),
        _ => anyhow::bail!("unsupported config format: .{ext}"),
    }
}

fn parse_config_value(raw: &str, path: &Path) -> anyhow::Result<serde_json::Value> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("toml");

    match ext {
        "toml" => {
            let v: toml::Value = toml::from_str(raw)?;
            Ok(serde_json::to_value(v)?)
        }
        "yaml" | "yml" => {
            let v: serde_yaml::Value = serde_yaml::from_str(raw)?;
            Ok(serde_json::to_value(v)?)
        }
        "json" => Ok(serde_json::from_str(raw)?),
        _ => anyhow::bail!("unsupported config format: .{ext}"),
    }
}
