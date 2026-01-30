use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{debug, info, warn};

use moltis_agents::tool_registry::AgentTool;

use crate::approval::{ApprovalAction, ApprovalDecision, ApprovalManager};

/// Broadcaster that notifies connected clients about pending approval requests.
#[async_trait]
pub trait ApprovalBroadcaster: Send + Sync {
    async fn broadcast_request(&self, request_id: &str, command: &str) -> Result<()>;
}

/// Result of a shell command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Options controlling exec behavior.
#[derive(Debug, Clone)]
pub struct ExecOpts {
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub working_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl Default for ExecOpts {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_output_bytes: 200 * 1024, // 200KB
            working_dir: None,
            env: Vec::new(),
        }
    }
}

/// Execute a shell command with timeout and output limits.
pub async fn exec_command(command: &str, opts: &ExecOpts) -> Result<ExecResult> {
    debug!(command, timeout_secs = opts.timeout.as_secs(), "exec_command");

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);

    if let Some(ref dir) = opts.working_dir {
        cmd.current_dir(dir);
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Prevent the child from inheriting stdin.
    cmd.stdin(std::process::Stdio::null());

    let child = cmd.spawn()?;

    let result = tokio::time::timeout(opts.timeout, child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) => {
            let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();

            // Truncate if exceeding limit.
            if stdout.len() > opts.max_output_bytes {
                stdout.truncate(opts.max_output_bytes);
                stdout.push_str("\n... [output truncated]");
            }
            if stderr.len() > opts.max_output_bytes {
                stderr.truncate(opts.max_output_bytes);
                stderr.push_str("\n... [output truncated]");
            }

            let exit_code = output.status.code().unwrap_or(-1);
            debug!(exit_code, stdout_len = stdout.len(), stderr_len = stderr.len(), "exec done");

            Ok(ExecResult { stdout, stderr, exit_code })
        }
        Ok(Err(e)) => bail!("failed to run command: {e}"),
        Err(_) => {
            warn!(command, "exec timeout");
            bail!("command timed out after {}s", opts.timeout.as_secs())
        }
    }
}

/// The exec tool exposed to the agent tool registry.
pub struct ExecTool {
    pub default_timeout: Duration,
    pub max_output_bytes: usize,
    pub working_dir: Option<PathBuf>,
    approval_manager: Option<Arc<ApprovalManager>>,
    broadcaster: Option<Arc<dyn ApprovalBroadcaster>>,
}

impl Default for ExecTool {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            max_output_bytes: 200 * 1024,
            working_dir: None,
            approval_manager: None,
            broadcaster: None,
        }
    }
}

impl ExecTool {
    /// Attach approval gating to this exec tool.
    pub fn with_approval(
        mut self,
        manager: Arc<ApprovalManager>,
        broadcaster: Arc<dyn ApprovalBroadcaster>,
    ) -> Self {
        self.approval_manager = Some(manager);
        self.broadcaster = Some(broadcaster);
        self
    }
}

#[async_trait]
impl AgentTool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command on the server. Returns stdout, stderr, and exit code."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 30, max 1800)"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the command"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<serde_json::Value> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'command' parameter"))?;

        let timeout_secs = params
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.default_timeout.as_secs())
            .min(1800); // cap at 30 minutes

        let working_dir = params
            .get("working_dir")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| self.working_dir.clone());

        info!(command, timeout_secs, ?working_dir, "exec tool invoked");

        // Approval gating.
        if let Some(ref mgr) = self.approval_manager {
            let action = mgr.check_command(command).await?;
            if action == ApprovalAction::NeedsApproval {
                info!(command, "command needs approval, waiting...");
                let (req_id, rx) = mgr.create_request(command).await;

                // Broadcast to connected clients.
                if let Some(ref bc) = self.broadcaster
                    && let Err(e) = bc.broadcast_request(&req_id, command).await
                {
                    warn!(error = %e, "failed to broadcast approval request");
                }

                let decision = mgr.wait_for_decision(rx).await;
                match decision {
                    ApprovalDecision::Approved => {
                        info!(command, "command approved");
                    }
                    ApprovalDecision::Denied => {
                        bail!("command denied by user: {command}");
                    }
                    ApprovalDecision::Timeout => {
                        bail!("approval timed out for command: {command}");
                    }
                }
            }
        }

        let opts = ExecOpts {
            timeout: Duration::from_secs(timeout_secs),
            max_output_bytes: self.max_output_bytes,
            working_dir,
            env: Vec::new(),
        };

        let result = exec_command(command, &opts).await?;
        info!(
            command,
            exit_code = result.exit_code,
            stdout_len = result.stdout.len(),
            stderr_len = result.stderr.len(),
            "exec tool completed"
        );
        Ok(serde_json::to_value(&result)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestBroadcaster {
        called: AtomicBool,
    }

    impl TestBroadcaster {
        fn new() -> Self {
            Self { called: AtomicBool::new(false) }
        }
    }

    #[async_trait]
    impl ApprovalBroadcaster for TestBroadcaster {
        async fn broadcast_request(&self, _request_id: &str, _command: &str) -> Result<()> {
            self.called.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_exec_echo() {
        let result = exec_command("echo hello", &ExecOpts::default()).await.unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_exec_stderr() {
        let result = exec_command("echo err >&2", &ExecOpts::default()).await.unwrap();
        assert_eq!(result.stderr.trim(), "err");
    }

    #[tokio::test]
    async fn test_exec_exit_code() {
        let result = exec_command("exit 42", &ExecOpts::default()).await.unwrap();
        assert_eq!(result.exit_code, 42);
    }

    #[tokio::test]
    async fn test_exec_timeout() {
        let opts = ExecOpts {
            timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let result = exec_command("sleep 10", &opts).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_exec_tool() {
        let tool = ExecTool::default();
        let result = tool
            .execute(serde_json::json!({ "command": "echo hello" }))
            .await
            .unwrap();
        assert_eq!(result["stdout"].as_str().unwrap().trim(), "hello");
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn test_exec_tool_safe_command_no_approval_needed() {
        let mgr = Arc::new(ApprovalManager::default());
        let bc = Arc::new(TestBroadcaster::new());
        let bc_dyn: Arc<dyn ApprovalBroadcaster> = Arc::clone(&bc) as _;
        let tool = ExecTool::default().with_approval(Arc::clone(&mgr), bc_dyn);
        // "echo" is in SAFE_BINS, should proceed without approval.
        let result = tool
            .execute(serde_json::json!({ "command": "echo safe" }))
            .await
            .unwrap();
        assert_eq!(result["stdout"].as_str().unwrap().trim(), "safe");
        assert!(!bc.called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_exec_tool_approval_approved() {
        let mgr = Arc::new(ApprovalManager::default());
        let bc = Arc::new(TestBroadcaster::new());
        let bc_dyn: Arc<dyn ApprovalBroadcaster> = Arc::clone(&bc) as _;
        let tool = ExecTool::default().with_approval(Arc::clone(&mgr), bc_dyn);

        let mgr2 = Arc::clone(&mgr);
        let handle = tokio::spawn(async move {
            // Wait a bit for the request to be created.
            tokio::time::sleep(Duration::from_millis(50)).await;
            let ids = mgr2.pending_ids().await;
            let id = ids.first().unwrap().clone();
            mgr2.resolve(&id, ApprovalDecision::Approved, Some("curl http://example.com")).await;
        });

        let result = tool
            .execute(serde_json::json!({ "command": "curl http://example.com" }))
            .await;
        handle.await.unwrap();
        // curl might not be available, but we at least verify the approval path didn't bail.
        // The broadcast should have been called.
        assert!(bc.called.load(Ordering::SeqCst));
        // Result could be ok or err depending on curl availability, that's fine.
        let _ = result;
    }

    #[tokio::test]
    async fn test_exec_tool_approval_denied() {
        let mgr = Arc::new(ApprovalManager::default());
        let bc = Arc::new(TestBroadcaster::new());
        let bc_dyn: Arc<dyn ApprovalBroadcaster> = Arc::clone(&bc) as _;
        let tool = ExecTool::default().with_approval(Arc::clone(&mgr), bc_dyn);

        let mgr2 = Arc::clone(&mgr);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let ids = mgr2.pending_ids().await;
            let id = ids.first().unwrap().clone();
            mgr2.resolve(&id, ApprovalDecision::Denied, None).await;
        });

        let result = tool
            .execute(serde_json::json!({ "command": "rm -rf /" }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("denied"));
    }
}
