//! MCP client: manages the protocol handshake and tool interactions with a single MCP server.

use std::{collections::HashMap, sync::Arc};

use {
    anyhow::{Context, Result},
    tracing::{debug, info},
};

use crate::{
    transport::StdioTransport,
    types::{
        ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, McpToolDef,
        PROTOCOL_VERSION, ToolsCallParams, ToolsCallResult, ToolsListResult,
    },
};

/// State of an MCP client connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientState {
    /// Transport spawned, not yet initialized.
    Connected,
    /// `initialize` completed, `initialized` notification sent.
    Ready,
    /// Server process exited or was shut down.
    Closed,
}

/// An MCP client connected to a single server via stdio.
pub struct McpClient {
    server_name: String,
    transport: Arc<StdioTransport>,
    state: McpClientState,
    server_info: Option<InitializeResult>,
    tools: Vec<McpToolDef>,
}

impl McpClient {
    /// Spawn the server process and perform the MCP handshake (initialize + initialized).
    pub async fn connect(
        server_name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let transport = StdioTransport::spawn(command, args, env).await?;

        let mut client = Self {
            server_name: server_name.into(),
            transport,
            state: McpClientState::Connected,
            server_info: None,
            tools: Vec::new(),
        };

        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&mut self) -> Result<()> {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.into(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "moltis".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        };

        let resp = self
            .transport
            .request("initialize", Some(serde_json::to_value(&params)?))
            .await
            .context("MCP initialize request failed")?;

        let result: InitializeResult =
            serde_json::from_value(resp.result.context("MCP initialize returned no result")?)
                .context("failed to parse MCP initialize result")?;

        info!(
            server = %self.server_name,
            protocol = %result.protocol_version,
            server_name = %result.server_info.name,
            "MCP server initialized"
        );

        self.server_info = Some(result);

        // Send `initialized` notification to complete handshake.
        self.transport
            .notify("notifications/initialized", None)
            .await?;
        self.state = McpClientState::Ready;

        Ok(())
    }

    /// Fetch the list of tools from the server.
    pub async fn list_tools(&mut self) -> Result<&[McpToolDef]> {
        self.ensure_ready()?;

        let resp = self.transport.request("tools/list", None).await?;
        let result: ToolsListResult =
            serde_json::from_value(resp.result.context("tools/list returned no result")?)?;

        debug!(
            server = %self.server_name,
            count = result.tools.len(),
            "fetched MCP tools"
        );

        self.tools = result.tools;
        Ok(&self.tools)
    }

    /// Call a tool on the server.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolsCallResult> {
        self.ensure_ready()?;

        let params = ToolsCallParams {
            name: name.into(),
            arguments,
        };

        let resp = self
            .transport
            .request("tools/call", Some(serde_json::to_value(&params)?))
            .await?;

        let result: ToolsCallResult =
            serde_json::from_value(resp.result.context("tools/call returned no result")?)?;

        Ok(result)
    }

    /// Get the cached list of tools (call `list_tools` first).
    pub fn tools(&self) -> &[McpToolDef] {
        &self.tools
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn state(&self) -> McpClientState {
        self.state
    }

    /// Check if the server process is still running.
    pub async fn is_alive(&self) -> bool {
        self.transport.is_alive().await
    }

    /// Shut down the server.
    pub async fn shutdown(&mut self) {
        self.state = McpClientState::Closed;
        self.transport.kill().await;
    }

    fn ensure_ready(&self) -> Result<()> {
        if self.state != McpClientState::Ready {
            anyhow::bail!(
                "MCP client for '{}' is not ready (state: {:?})",
                self.server_name,
                self.state
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_state_debug() {
        assert_eq!(format!("{:?}", McpClientState::Connected), "Connected");
        assert_eq!(format!("{:?}", McpClientState::Ready), "Ready");
        assert_eq!(format!("{:?}", McpClientState::Closed), "Closed");
    }
}
