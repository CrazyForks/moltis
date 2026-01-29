use std::path::PathBuf;

/// Returns `~/.config/moltis` on all platforms.
pub fn moltis_config_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("moltis")
}
