/// PTY-based shell execution for the bash tool.
pub async fn exec_command(_command: &str, _cwd: &std::path::Path) -> anyhow::Result<ExecResult> {
    todo!("spawn PTY process, capture output, handle timeout/abort")
}

pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
