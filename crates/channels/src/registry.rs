use std::collections::HashMap;
use super::plugin::ChannelPlugin;

/// Registry of all loaded channel plugins.
pub struct ChannelRegistry {
    plugins: HashMap<String, Box<dyn ChannelPlugin>>,
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn ChannelPlugin>) {
        self.plugins.insert(plugin.id().to_string(), plugin);
    }

    pub fn get(&self, id: &str) -> Option<&dyn ChannelPlugin> {
        self.plugins.get(id).map(|p| p.as_ref())
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Box<dyn ChannelPlugin>> {
        self.plugins.get_mut(id)
    }

    pub fn list(&self) -> Vec<&str> {
        self.plugins.keys().map(|s| s.as_str()).collect()
    }
}
