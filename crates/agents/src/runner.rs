use anyhow::Result;

/// Run an agent: build prompt, invoke LLM, execute tool calls, stream response.
pub async fn run_agent(
    _agent_id: &str,
    _session_key: &str,
    _message: &str,
) -> Result<String> {
    todo!("build system prompt, load session, call LLM with tools, handle tool loop, return final text")
}
