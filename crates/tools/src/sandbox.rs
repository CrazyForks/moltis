/// Sandbox mode: restricted workspace, no network, no system dirs.
pub struct SandboxConfig {
    pub workspace: std::path::PathBuf,
    pub allowed_tools: Vec<String>,
}
