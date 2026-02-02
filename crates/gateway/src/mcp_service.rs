//! Live MCP service implementation backed by `McpManager`.

use std::sync::Arc;

use {async_trait::async_trait, serde_json::Value, tracing::info};

use crate::services::{McpService, ServiceResult};

/// Live MCP service delegating to `McpManager`.
pub struct LiveMcpService {
    manager: Arc<moltis_mcp::McpManager>,
}

impl LiveMcpService {
    pub fn new(manager: Arc<moltis_mcp::McpManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl McpService for LiveMcpService {
    async fn list(&self) -> ServiceResult {
        let statuses = self.manager.status_all().await;
        serde_json::to_value(&statuses).map_err(|e| e.to_string())
    }

    async fn add(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'command' parameter".to_string())?;
        let args: Vec<String> = params
            .get("args")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let env: std::collections::HashMap<String, String> = params
            .get("env")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let enabled = params
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let config = moltis_mcp::McpServerConfig {
            command: command.into(),
            args,
            env,
            enabled,
        };

        info!(server = %name, "adding MCP server via API");
        self.manager
            .add_server(name.into(), config, true)
            .await
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({ "ok": true }))
    }

    async fn remove(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        let removed = self
            .manager
            .remove_server(name)
            .await
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({ "removed": removed }))
    }

    async fn enable(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        let ok = self
            .manager
            .enable_server(name)
            .await
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({ "enabled": ok }))
    }

    async fn disable(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        let ok = self
            .manager
            .disable_server(name)
            .await
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({ "disabled": ok }))
    }

    async fn status(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        match self.manager.status(name).await {
            Some(s) => serde_json::to_value(&s).map_err(|e| e.to_string()),
            None => Err(format!("MCP server '{name}' not found")),
        }
    }

    async fn tools(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        match self.manager.server_tools(name).await {
            Some(tools) => serde_json::to_value(&tools).map_err(|e| e.to_string()),
            None => Err(format!("MCP server '{name}' not found or not running")),
        }
    }

    async fn restart(&self, params: Value) -> ServiceResult {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'name' parameter".to_string())?;

        self.manager
            .restart_server(name)
            .await
            .map_err(|e| e.to_string())?;

        Ok(serde_json::json!({ "ok": true }))
    }
}
