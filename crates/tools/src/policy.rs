/// Multi-layered tool policy resolution.
///
/// Layers (checked in order):
/// 1. Global: tools.policy.allow / deny
/// 2. Per-agent: agents.list[].tools.policy
/// 3. Per-model-provider: tools.providers.<provider>.policy
/// 4. Per-group: channels.<ch>.groups.<gid>.tools.policy
/// 5. Per-sender: channels.<ch>.groups.<gid>.tools.bySender.<sender>.allow
/// 6. Sandbox: tools.exec.sandbox.tools
pub struct ToolPolicy {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

pub fn resolve_effective_policy(_config: &serde_json::Value, _context: &PolicyContext) -> ToolPolicy {
    todo!("merge policy layers in precedence order")
}

pub struct PolicyContext {
    pub agent_id: String,
    pub provider: Option<String>,
    pub channel: Option<String>,
    pub group_id: Option<String>,
    pub sender_id: Option<String>,
    pub sandboxed: bool,
}
