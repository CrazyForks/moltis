/// Load and parse the config file with env substitution and includes.
pub fn load_config(_path: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    todo!("load JSON5 config, apply env substitution, resolve $include directives")
}
