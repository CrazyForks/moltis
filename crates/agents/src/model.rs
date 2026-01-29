use async_trait::async_trait;

/// LLM provider trait (Anthropic, OpenAI, Google, etc.).
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn complete(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse>;
}

/// Response from an LLM completion call.
#[derive(Debug)]
pub struct CompletionResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

#[derive(Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Model selection with fallback chain.
pub fn select_model(_config: &serde_json::Value) -> anyhow::Result<Box<dyn LlmProvider>> {
    todo!("resolve model from config, build provider with fallback")
}
