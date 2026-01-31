/// Plugin API surface: what plugins can register.
///
/// registerTool, registerHook, registerChannel, registerProvider,
/// registerCommand, registerGatewayMethod, registerHttpRoute,
/// registerService, registerCli.
pub trait PluginApi {
    fn register_tool(&mut self, tool: Box<dyn moltis_agents::tool_registry::AgentTool>);
    fn register_channel(&mut self, channel: Box<dyn moltis_channels::ChannelPlugin>);
    fn register_skill(&mut self, metadata: moltis_skills::types::SkillMetadata);
}
