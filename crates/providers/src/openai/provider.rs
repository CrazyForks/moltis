use super::*;

use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
};

use {
    async_trait::async_trait,
    futures::{SinkExt, StreamExt},
    secrecy::ExposeSecret,
    tokio_stream::Stream,
    tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

use tracing::{debug, trace, warn};

use super::openai_compat::{
    ResponsesStreamState, SseLineResult, StreamingToolState, finalize_responses_stream,
    finalize_stream, parse_openai_compat_usage, parse_openai_compat_usage_from_payload,
    parse_tool_calls, process_openai_sse_line, process_responses_sse_line, responses_output_index,
    split_responses_instructions_and_input, strip_think_tags, to_openai_tools,
    to_responses_api_tools,
};

use moltis_agents::model::{
    ChatMessage, CompletionResponse, LlmProvider, ModelMetadata, StreamEvent, Usage,
};
impl OpenAiProvider {
    pub fn new(api_key: secrecy::Secret<String>, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            base_url,
            provider_name: "openai".into(),
            client: crate::shared_http_client(),
            stream_transport: ProviderStreamTransport::Sse,
            wire_api: WireApi::ChatCompletions,
            metadata_cache: tokio::sync::OnceCell::new(),
            tool_mode_override: None,
            reasoning_effort: None,
            cache_retention: moltis_config::CacheRetention::Short,
        }
    }

    pub fn new_with_name(
        api_key: secrecy::Secret<String>,
        model: String,
        base_url: String,
        provider_name: String,
    ) -> Self {
        Self {
            api_key,
            model,
            base_url,
            provider_name,
            client: crate::shared_http_client(),
            stream_transport: ProviderStreamTransport::Sse,
            wire_api: WireApi::ChatCompletions,
            metadata_cache: tokio::sync::OnceCell::new(),
            tool_mode_override: None,
            reasoning_effort: None,
            cache_retention: moltis_config::CacheRetention::Short,
        }
    }

    #[must_use]
    pub fn with_cache_retention(mut self, cache_retention: moltis_config::CacheRetention) -> Self {
        self.cache_retention = cache_retention;
        self
    }

    #[must_use]
    pub fn with_stream_transport(mut self, stream_transport: ProviderStreamTransport) -> Self {
        self.stream_transport = stream_transport;
        self
    }

    #[must_use]
    pub fn with_tool_mode(mut self, mode: moltis_config::ToolMode) -> Self {
        self.tool_mode_override = Some(mode);
        self
    }

    #[must_use]
    pub fn with_wire_api(mut self, wire_api: WireApi) -> Self {
        self.wire_api = wire_api;
        self
    }

    /// Return the reasoning effort string if configured.
    fn reasoning_effort_str(&self) -> Option<&'static str> {
        use moltis_agents::model::ReasoningEffort;
        self.reasoning_effort.map(|e| match e {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        })
    }

    /// Apply `reasoning_effort` for the **Chat Completions** API (used by
    /// `complete()` and `stream_with_tools_sse()`).
    ///
    /// Format: `"reasoning_effort": "high"` (top-level string field).
    fn apply_reasoning_effort_chat(&self, body: &mut serde_json::Value) {
        if let Some(effort) = self.reasoning_effort_str() {
            body["reasoning_effort"] = serde_json::json!(effort);
        }
    }

    /// Apply `reasoning_effort` for the **Responses** API (used by
    /// `stream_with_tools_websocket()`).
    ///
    /// Format: `"reasoning": { "effort": "high" }` (nested object).
    fn apply_reasoning_effort_responses(&self, body: &mut serde_json::Value) {
        if let Some(effort) = self.reasoning_effort_str() {
            body["reasoning"] = serde_json::json!({ "effort": effort });
        }
    }

    fn apply_probe_output_cap_chat(&self, body: &mut serde_json::Value) {
        let raw = raw_model_id(&self.model).to_ascii_lowercase();
        let capability = raw.rsplit('/').next().unwrap_or(raw.as_str());
        let uses_max_completion_tokens = capability.starts_with("gpt-5")
            || capability.starts_with("o1")
            || capability.starts_with("o3")
            || capability.starts_with("o4");
        if uses_max_completion_tokens {
            // GPT-5 and reasoning models need a higher minimum output cap.
            // Values below ~10 can trigger 400 errors on some models.
            body["max_completion_tokens"] = serde_json::json!(16);
        } else {
            body["max_tokens"] = serde_json::json!(1);
        }
    }

    async fn probe_chat_completions(&self) -> anyhow::Result<()> {
        let messages = vec![ChatMessage::user("ping")];
        let mut openai_messages = self.serialize_messages_for_request(&messages);
        self.apply_openrouter_cache_control(&mut openai_messages);
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": openai_messages,
        });
        self.apply_system_prompt_rewrite(&mut body);
        // Probes only answer "can this model respond at all?".
        // Keep them cheap instead of mirroring full reasoning budgets.
        self.apply_probe_output_cap_chat(&mut body);

        debug!(model = %self.model, "openai probe request");
        trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "openai probe request body");

        let http_resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = http_resp.status();
        if !status.is_success() {
            let retry_after_ms = retry_after_ms_from_headers(http_resp.headers());
            let body_text = http_resp.text().await.unwrap_or_default();
            if should_warn_on_api_error(status, &body_text) {
                warn!(
                    status = %status,
                    model = %self.model,
                    provider = %self.provider_name,
                    body = %body_text,
                    "openai probe API error"
                );
            } else {
                debug!(
                    status = %status,
                    model = %self.model,
                    provider = %self.provider_name,
                    "openai probe model unsupported for chat/completions endpoint"
                );
            }
            // Ollama's OpenAI-compat layer returns 404 for models that
            // exist but aren't wired to /v1/chat/completions.  Fall back
            // to the native `/api/show` endpoint before giving up.
            if status == reqwest::StatusCode::NOT_FOUND
                && self.provider_name.eq_ignore_ascii_case("ollama")
            {
                return self.probe_ollama_native().await;
            }

            anyhow::bail!(
                "{}",
                with_retry_after_marker(
                    format!("OpenAI API error HTTP {status}: {body_text}"),
                    retry_after_ms,
                )
            );
        }

        Ok(())
    }

    /// Fallback probe for Ollama: POST `/api/show` with the model name.
    ///
    /// This confirms the model is installed and Ollama is reachable even when
    /// the OpenAI-compat `/v1/chat/completions` endpoint returns 404.
    async fn probe_ollama_native(&self) -> anyhow::Result<()> {
        let api_base = normalize_ollama_api_base_url(&self.base_url);
        let url = format!("{}/api/show", api_base.trim_end_matches('/'));

        debug!(model = %self.model, url = %url, "ollama native probe via /api/show");

        let mut req = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "name": self.model }));
        let key = self.api_key.expose_secret();
        if !key.is_empty() {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send().await?;

        if resp.status().is_success() {
            return Ok(());
        }

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Model '{}' not found. Make sure it is installed (ollama pull {}) \
             and try again. (Ollama /api/show returned HTTP {}: {})",
            self.model,
            self.model,
            status,
            body_text,
        )
    }

    async fn probe_responses(&self) -> anyhow::Result<()> {
        let messages = vec![ChatMessage::user("ping")];
        let (instructions, input) = split_responses_instructions_and_input(messages);
        let mut body = serde_json::json!({
            "model": self.model,
            "input": input,
            "max_output_tokens": 1,
        });

        if let Some(instructions) = instructions {
            body["instructions"] = serde_json::Value::String(instructions);
        }

        self.apply_reasoning_effort_responses(&mut body);

        debug!(model = %self.model, "openai responses probe request");
        trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "openai responses probe request body");

        let url = self.responses_sse_url();
        let http_resp = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = http_resp.status();
        if !status.is_success() {
            let retry_after_ms = retry_after_ms_from_headers(http_resp.headers());
            let body_text = http_resp.text().await.unwrap_or_default();
            warn!(
                status = %status,
                model = %self.model,
                provider = %self.provider_name,
                body = %body_text,
                "openai responses probe API error"
            );
            anyhow::bail!(
                "{}",
                with_retry_after_marker(
                    format!("OpenAI API error HTTP {status}: {body_text}"),
                    retry_after_ms,
                )
            );
        }

        Ok(())
    }

    fn requires_reasoning_content_on_tool_messages(&self) -> bool {
        self.provider_name.eq_ignore_ascii_case("moonshot")
            || self.base_url.contains("moonshot.ai")
            || self.base_url.contains("moonshot.cn")
            || self.model.starts_with("kimi-")
    }

    /// Some providers (e.g. MiniMax) reject `role: "system"` in the messages
    /// array. System content must be extracted and prepended to the first user
    /// message instead (MiniMax silently ignores a top-level `"system"` field).
    fn rejects_system_role(&self) -> bool {
        self.model.starts_with("MiniMax-")
            || self.provider_name.eq_ignore_ascii_case("minimax")
            || self.base_url.to_ascii_lowercase().contains("minimax")
    }

    /// For providers that reject `role: "system"` in the messages array,
    /// extract all system messages from `body["messages"]`, join their
    /// content, and prepend it to the first user message.
    ///
    /// MiniMax's `/v1/chat/completions` endpoint returns error 2013 for
    /// `role: "system"` entries and silently ignores a top-level `"system"`
    /// field. The only reliable way to deliver the system prompt is to
    /// inline it into the first user message.
    ///
    /// Must be called on the request body **after** it is fully assembled.
    fn apply_system_prompt_rewrite(&self, body: &mut serde_json::Value) {
        if !self.rejects_system_role() {
            return;
        }
        let Some(messages) = body
            .get_mut("messages")
            .and_then(serde_json::Value::as_array_mut)
        else {
            return;
        };
        let mut system_parts = Vec::new();
        messages.retain(|msg| {
            if msg.get("role").and_then(serde_json::Value::as_str) == Some("system") {
                if let Some(content) = msg.get("content").and_then(serde_json::Value::as_str)
                    && !content.is_empty()
                {
                    system_parts.push(content.to_string());
                } else if msg.get("content").is_some() {
                    warn!("MiniMax system message has non-string content; it will be dropped");
                }
                return false;
            }
            true
        });
        if system_parts.is_empty() {
            return;
        }
        let system_text = system_parts.join("\n\n");

        // Find the first user message and prepend system content to it.
        let system_block =
            format!("[System Instructions]\n{system_text}\n[End System Instructions]\n\n");
        if let Some(first_user) = messages
            .iter_mut()
            .find(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        {
            match first_user.get("content").cloned() {
                Some(serde_json::Value::String(s)) => {
                    first_user["content"] = serde_json::Value::String(format!("{system_block}{s}"));
                },
                Some(serde_json::Value::Array(mut arr)) => {
                    // Multimodal content (text + images): prepend as a text block.
                    arr.insert(
                        0,
                        serde_json::json!({ "type": "text", "text": system_block }),
                    );
                    first_user["content"] = serde_json::Value::Array(arr);
                },
                _ => {
                    first_user["content"] = serde_json::Value::String(system_block);
                },
            }
        } else {
            // No user message yet (e.g. probe); insert a synthetic user message.
            messages.insert(
                0,
                serde_json::json!({
                    "role": "user",
                    "content": format!("[System Instructions]\n{system_text}\n[End System Instructions]")
                }),
            );
        }
    }

    fn serialize_messages_for_request(&self, messages: &[ChatMessage]) -> Vec<serde_json::Value> {
        let needs_reasoning_content = self.requires_reasoning_content_on_tool_messages();
        let mut remapped_tool_call_ids = HashMap::new();
        let mut used_tool_call_ids = HashSet::new();
        let mut out = Vec::with_capacity(messages.len());

        for message in messages {
            let mut value = message.to_openai_value();

            if let Some(tool_calls) = value
                .get_mut("tool_calls")
                .and_then(serde_json::Value::as_array_mut)
            {
                for tool_call in tool_calls {
                    let Some(tool_call_id) =
                        tool_call.get("id").and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    let mapped_id = assign_openai_tool_call_id(
                        tool_call_id,
                        &mut remapped_tool_call_ids,
                        &mut used_tool_call_ids,
                    );
                    tool_call["id"] = serde_json::Value::String(mapped_id);
                }
            } else if value.get("role").and_then(serde_json::Value::as_str) == Some("tool")
                && let Some(tool_call_id) = value
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
            {
                let mapped_id = remapped_tool_call_ids
                    .get(tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        assign_openai_tool_call_id(
                            tool_call_id,
                            &mut remapped_tool_call_ids,
                            &mut used_tool_call_ids,
                        )
                    });
                value["tool_call_id"] = serde_json::Value::String(mapped_id);
            }

            if needs_reasoning_content {
                let is_assistant =
                    value.get("role").and_then(serde_json::Value::as_str) == Some("assistant");
                let has_tool_calls = value
                    .get("tool_calls")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|calls| !calls.is_empty());

                if is_assistant && has_tool_calls {
                    let reasoning_content = value
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();

                    if value.get("content").is_none() {
                        value["content"] = serde_json::Value::String(String::new());
                    }

                    if value.get("reasoning_content").is_none() {
                        value["reasoning_content"] = serde_json::Value::String(reasoning_content);
                    }
                }
            }

            out.push(value);
        }

        out
    }

    fn is_openai_platform_base_url(&self) -> bool {
        reqwest::Url::parse(&self.base_url)
            .ok()
            .and_then(|url| url.host_str().map(ToString::to_string))
            .is_some_and(|host| host.eq_ignore_ascii_case("api.openai.com"))
    }

    /// Returns `true` when this provider targets an Anthropic model via
    /// OpenRouter, which supports prompt caching when `cache_control`
    /// breakpoints are present in the message payload.
    fn is_openrouter_anthropic(&self) -> bool {
        self.base_url.contains("openrouter.ai") && self.model.starts_with("anthropic/")
    }

    /// For OpenRouter Anthropic models, inject `cache_control` breakpoints
    /// on the system message and the last user message to enable prompt
    /// caching passthrough to Anthropic.
    fn apply_openrouter_cache_control(&self, messages: &mut [serde_json::Value]) {
        if !self.is_openrouter_anthropic()
            || matches!(self.cache_retention, moltis_config::CacheRetention::None)
        {
            return;
        }

        let cache_control = serde_json::json!({ "type": "ephemeral" });

        // Add cache_control to the system message content.
        for msg in messages.iter_mut() {
            if msg.get("role").and_then(serde_json::Value::as_str) != Some("system") {
                continue;
            }
            match msg.get_mut("content") {
                Some(content) if content.is_string() => {
                    let text = content.as_str().unwrap_or_default().to_string();
                    msg["content"] = serde_json::json!([{
                        "type": "text",
                        "text": text,
                        "cache_control": cache_control
                    }]);
                },
                Some(content) if content.is_array() => {
                    if let Some(last) = content.as_array_mut().and_then(|a| a.last_mut()) {
                        last["cache_control"] = cache_control.clone();
                    }
                },
                _ => {},
            }
            break;
        }

        // Add cache_control to the last user message.
        if let Some(last_user) = messages
            .iter_mut()
            .rev()
            .find(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        {
            match last_user.get_mut("content") {
                Some(content) if content.is_string() => {
                    let text = content.as_str().unwrap_or_default().to_string();
                    last_user["content"] = serde_json::json!([{
                        "type": "text",
                        "text": text,
                        "cache_control": cache_control
                    }]);
                },
                Some(content) if content.is_array() => {
                    if let Some(last) = content.as_array_mut().and_then(|a| a.last_mut()) {
                        last["cache_control"] = cache_control;
                    }
                },
                _ => {},
            }
        }
    }

    /// Build the HTTP URL for the Responses API (`/responses`).
    ///
    /// If the base URL already ends with `/responses`, use it as-is.
    /// Otherwise derive it as a sibling of `/chat/completions`, ensuring
    /// `/v1` is present — matching the normalization in
    /// `responses_websocket_url`.
    fn responses_sse_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/responses") {
            return base.to_string();
        }
        if let Some(prefix) = base.strip_suffix("/chat/completions") {
            return format!("{prefix}/responses");
        }
        // Ensure /v1 is present, consistent with responses_websocket_url.
        if base.ends_with("/v1") {
            format!("{base}/responses")
        } else {
            format!("{base}/v1/responses")
        }
    }

    fn responses_websocket_url(&self) -> crate::error::Result<String> {
        let mut base = self.base_url.trim_end_matches('/').to_string();
        if !base.ends_with("/v1") {
            base.push_str("/v1");
        }
        let url = format!("{base}/responses");
        if let Some(rest) = url.strip_prefix("https://") {
            return Ok(format!("wss://{rest}"));
        }
        if let Some(rest) = url.strip_prefix("http://") {
            return Ok(format!("ws://{rest}"));
        }
        Err(crate::error::Error::message(format!(
            "invalid OpenAI base_url for websocket mode: expected http:// or https://, got {}",
            self.base_url
        )))
    }

    /// Stream using the OpenAI Responses API format (`/responses`) over SSE.
    #[allow(clippy::collapsible_if)]
    fn stream_responses_sse(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        Box::pin(async_stream::stream! {
            let (instructions, input) = split_responses_instructions_and_input(messages);
            let mut body = serde_json::json!({
                "model": self.model,
                "input": input,
                "stream": true,
            });

            if let Some(instructions) = instructions {
                body["instructions"] = serde_json::Value::String(instructions);
            }

            if !tools.is_empty() {
                body["tools"] = serde_json::Value::Array(to_responses_api_tools(&tools));
                body["tool_choice"] = serde_json::json!("auto");
            }

            self.apply_reasoning_effort_responses(&mut body);

            debug!(
                model = %self.model,
                tools_count = tools.len(),
                "openai stream_responses_sse request"
            );
            trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "openai responses stream request body");

            let url = self.responses_sse_url();
            let resp = match self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key.expose_secret()))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => {
                    if let Err(e) = r.error_for_status_ref() {
                        let status = e.status().map(|s| s.as_u16()).unwrap_or(0);
                        let retry_after_ms = retry_after_ms_from_headers(r.headers());
                        let body_text = r.text().await.unwrap_or_default();
                        yield StreamEvent::Error(with_retry_after_marker(
                            format!("HTTP {status}: {body_text}"),
                            retry_after_ms,
                        ));
                        return;
                    }
                    r
                }
                Err(e) => {
                    yield StreamEvent::Error(e.to_string());
                    return;
                }
            };

            let mut byte_stream = resp.bytes_stream();
            let mut buf = String::new();
            let mut state = ResponsesStreamState::default();
            let mut stream_done = false;

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield StreamEvent::Error(e.to_string());
                        return;
                    }
                };
                buf.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim().to_string();
                    buf = buf[pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    let Some(data) = line
                        .strip_prefix("data: ")
                        .or_else(|| line.strip_prefix("data:"))
                    else {
                        // Handle bare event types (e.g. "event: response.completed")
                        continue;
                    };

                    match process_responses_sse_line(data, &mut state) {
                        SseLineResult::Done => {
                            stream_done = true;
                            break;
                        }
                        SseLineResult::Events(events) => {
                            for event in events {
                                yield event;
                            }
                        }
                        SseLineResult::Skip => {}
                    }
                }
                if stream_done {
                    break;
                }
            }

            // Process any residual buffered line on EOF.
            if !stream_done {
                let line = buf.trim().to_string();
                if !line.is_empty()
                    && let Some(data) = line
                        .strip_prefix("data: ")
                        .or_else(|| line.strip_prefix("data:"))
                {
                    match process_responses_sse_line(data, &mut state) {
                        SseLineResult::Done | SseLineResult::Skip => {}
                        SseLineResult::Events(events) => {
                            for event in events {
                                yield event;
                            }
                        }
                    }
                }
            }

            // Finalize: emit pending ToolCallComplete events + Done with usage.
            for event in finalize_responses_stream(&mut state) {
                yield event;
            }
        })
    }

    #[allow(clippy::collapsible_if)]
    fn stream_with_tools_sse(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        Box::pin(async_stream::stream! {
            let mut openai_messages = self.serialize_messages_for_request(&messages);
            self.apply_openrouter_cache_control(&mut openai_messages);
            let mut body = serde_json::json!({
                "model": self.model,
                "messages": openai_messages,
                "stream": true,
                "stream_options": { "include_usage": true },
            });
            self.apply_system_prompt_rewrite(&mut body);

            if !tools.is_empty() {
                body["tools"] = serde_json::Value::Array(to_openai_tools(&tools));
            }

            self.apply_reasoning_effort_chat(&mut body);

            debug!(
                model = %self.model,
                messages_count = openai_messages.len(),
                tools_count = tools.len(),
                reasoning_effort = ?self.reasoning_effort,
                "openai stream_with_tools request (sse)"
            );
            trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "openai stream request body (sse)");

            let resp = match self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key.expose_secret()))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
            {
                Ok(r) => {
                    if let Err(e) = r.error_for_status_ref() {
                        let status = e.status().map(|s| s.as_u16()).unwrap_or(0);
                        let retry_after_ms = retry_after_ms_from_headers(r.headers());
                        let body_text = r.text().await.unwrap_or_default();
                        yield StreamEvent::Error(with_retry_after_marker(
                            format!("HTTP {status}: {body_text}"),
                            retry_after_ms,
                        ));
                        return;
                    }
                    r
                }
                Err(e) => {
                    yield StreamEvent::Error(e.to_string());
                    return;
                }
            };

            let mut byte_stream = resp.bytes_stream();
            let mut buf = String::new();
            let mut state = StreamingToolState::default();

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield StreamEvent::Error(e.to_string());
                        return;
                    }
                };
                buf.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim().to_string();
                    buf = buf[pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    let Some(data) = line
                        .strip_prefix("data: ")
                        .or_else(|| line.strip_prefix("data:"))
                    else {
                        continue;
                    };

                    match process_openai_sse_line(data, &mut state) {
                        SseLineResult::Done => {
                            for event in finalize_stream(&mut state) {
                                yield event;
                            }
                            return;
                        }
                        SseLineResult::Events(events) => {
                            for event in events {
                                yield event;
                            }
                        }
                        SseLineResult::Skip => {}
                    }
                }
            }

            // Some OpenAI-compatible providers may close the stream without
            // an explicit [DONE] frame or trailing newline. Process any
            // residual buffered line and always finalize on EOF so usage
            // metadata still propagates.
            let line = buf.trim().to_string();
            if !line.is_empty()
                && let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
            {
                match process_openai_sse_line(data, &mut state) {
                    SseLineResult::Done => {
                        for event in finalize_stream(&mut state) {
                            yield event;
                        }
                        return;
                    }
                    SseLineResult::Events(events) => {
                        for event in events {
                            yield event;
                        }
                    }
                    SseLineResult::Skip => {}
                }
            }

            for event in finalize_stream(&mut state) {
                yield event;
            }
        })
    }

    #[allow(clippy::collapsible_if)]
    fn stream_with_tools_websocket(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
        fallback_to_sse: bool,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        // Synchronous pre-flight: URL, request, auth header, pool key.
        // Fail fast and fall back to SSE before entering the async generator,
        // which avoids cloning messages/tools for the four sync-check paths.
        let (request, pool_key) = match (|| -> crate::error::Result<_> {
            if !self.is_openai_platform_base_url() {
                return Err(crate::error::Error::message(format!(
                    "websocket mode is only supported for api.openai.com (got {})",
                    self.base_url
                )));
            }
            let ws_url = self.responses_websocket_url()?;
            let pk = ws_pool::PoolKey::new(&ws_url, &self.api_key);
            let mut req = ws_url.as_str().into_client_request()?;
            let auth = format!("Bearer {}", self.api_key.expose_secret());
            req.headers_mut()
                .insert("Authorization", HeaderValue::from_str(&auth)?);
            req.headers_mut()
                .insert("OpenAI-Beta", HeaderValue::from_static("responses=v1"));
            Ok((req, pk))
        })() {
            Ok(r) => r,
            Err(err) => {
                if fallback_to_sse {
                    debug!(error = %err, "websocket setup failed, falling back to sse");
                    return self.stream_with_tools_sse(messages, tools);
                }
                return Box::pin(async_stream::stream! {
                    yield StreamEvent::Error(err.to_string());
                });
            },
        };

        Box::pin(async_stream::stream! {
            // Try the pool first; fall back to a fresh connection.
            let (mut ws_stream, created_at) = if let Some(pooled) = ws_pool::shared_ws_pool().checkout(&pool_key).await {
                pooled
            } else {
                match tokio_tungstenite::connect_async(request).await {
                    Ok((ws, _)) => (ws, std::time::Instant::now()),
                    Err(err) => {
                        if fallback_to_sse {
                            debug!(error = %err, "websocket connect failed, falling back to sse");
                            let mut sse = self.stream_with_tools_sse(messages, tools);
                            while let Some(event) = sse.next().await {
                                yield event;
                            }
                        } else {
                            yield StreamEvent::Error(err.to_string());
                        }
                        return;
                    }
                }
            };

            let (instructions, input) = split_responses_instructions_and_input(messages);
            let mut response_payload = serde_json::json!({
                "model": self.model,
                "stream": true,
                "store": false,
                "input": input,
            });
            if let Some(instructions) = instructions {
                response_payload["instructions"] = serde_json::Value::String(instructions);
            }
            if !tools.is_empty() {
                response_payload["tools"] = serde_json::Value::Array(to_responses_api_tools(&tools));
                response_payload["tool_choice"] = serde_json::json!("auto");
            }

            self.apply_reasoning_effort_responses(&mut response_payload);

            let create_event = serde_json::json!({
                "type": "response.create",
                "response": response_payload,
            });

            debug!(
                model = %self.model,
                tools_count = tools.len(),
                reasoning_effort = ?self.reasoning_effort,
                "openai stream_with_tools request (websocket)"
            );
            trace!(event = %create_event, "openai websocket create event");

            if let Err(err) = ws_stream
                .send(Message::Text(create_event.to_string().into()))
                .await
            {
                yield StreamEvent::Error(format!("websocket send failed: {err}"));
                return;
            }

            let mut input_tokens: u32 = 0;
            let mut output_tokens: u32 = 0;
            let mut cache_read_tokens: u32 = 0;
            let mut cache_write_tokens: u32 = 0;
            let mut current_tool_index: usize = 0;
            let mut tool_calls: HashMap<usize, (String, String)> = HashMap::new();
            let mut completed_tool_calls: HashSet<usize> = HashSet::new();
            let mut clean_completion = false;

            while let Some(frame) = ws_stream.next().await {
                let text = match frame {
                    Ok(Message::Text(t)) => t.to_string(),
                    Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).into_owned(),
                    Ok(Message::Ping(p)) => {
                        if let Err(err) = ws_stream.send(Message::Pong(p)).await {
                            yield StreamEvent::Error(err.to_string());
                            return;
                        }
                        continue;
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => continue,
                    Err(err) => {
                        yield StreamEvent::Error(err.to_string());
                        return;
                    }
                };

                let Ok(evt) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                trace!(event = %evt, "openai websocket event");

                match evt["type"].as_str().unwrap_or("") {
                    "response.output_text.delta" => {
                        if let Some(delta) = evt["delta"].as_str()
                            && !delta.is_empty()
                        {
                            yield StreamEvent::Delta(delta.to_string());
                        }
                    }
                    "response.output_item.added" => {
                        if evt["item"]["type"].as_str() == Some("function_call") {
                            let id = evt["item"]["call_id"].as_str().unwrap_or("").to_string();
                            let name = evt["item"]["name"].as_str().unwrap_or("").to_string();
                            let index = responses_output_index(&evt, current_tool_index);
                            current_tool_index = current_tool_index.max(index + 1);
                            tool_calls.insert(index, (id.clone(), name.clone()));
                            yield StreamEvent::ToolCallStart { id, name, index };
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        if let Some(delta) = evt["delta"].as_str()
                            && !delta.is_empty()
                        {
                            let index = responses_output_index(&evt, current_tool_index.saturating_sub(1));
                            yield StreamEvent::ToolCallArgumentsDelta {
                                index,
                                delta: delta.to_string(),
                            };
                        }
                    }
                    "response.function_call_arguments.done" => {
                        let index = responses_output_index(&evt, current_tool_index.saturating_sub(1));
                        if completed_tool_calls.insert(index) {
                            yield StreamEvent::ToolCallComplete { index };
                        }
                    }
                    "response.completed" => {
                        if let Some(usage) = evt.get("response").and_then(|response| response.get("usage")) {
                            let parsed = parse_openai_compat_usage(usage);
                            input_tokens = parsed.input_tokens;
                            output_tokens = parsed.output_tokens;
                            cache_read_tokens = parsed.cache_read_tokens;
                            cache_write_tokens = parsed.cache_write_tokens;
                        }
                        let mut pending: Vec<usize> = tool_calls.keys().copied().collect();
                        pending.sort_unstable();
                        for index in pending {
                            if completed_tool_calls.insert(index) {
                                yield StreamEvent::ToolCallComplete { index };
                            }
                        }
                        clean_completion = true;
                        break;
                    }
                    "error" | "response.failed" => {
                        let msg = evt["error"]["message"]
                            .as_str()
                            .or_else(|| evt["response"]["error"]["message"].as_str())
                            .or_else(|| evt["message"].as_str())
                            .unwrap_or("unknown error");
                        yield StreamEvent::Error(msg.to_string());
                        return;
                    }
                    _ => {}
                }
            }

            // Emit any remaining tool-call completions (fallback for broken streams).
            if !clean_completion {
                let mut pending: Vec<usize> = tool_calls.keys().copied().collect();
                pending.sort_unstable();
                for index in pending {
                    if completed_tool_calls.insert(index) {
                        yield StreamEvent::ToolCallComplete { index };
                    }
                }
            }

            // Return healthy connections to the pool; drop on error / close.
            if clean_completion {
                ws_pool::shared_ws_pool()
                    .return_conn(pool_key, ws_stream, created_at)
                    .await;
            }

            yield StreamEvent::Done(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            });
        })
    }

    /// Non-streaming completion using the Responses API.
    ///
    /// Sends `stream: true` and collects events into a single response, since
    /// many Responses API endpoints only support streaming.
    async fn complete_responses(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        let (instructions, input) = split_responses_instructions_and_input(messages.to_vec());
        let mut body = serde_json::json!({
            "model": self.model,
            "input": input,
            "stream": true,
        });
        if let Some(instructions) = instructions {
            body["instructions"] = serde_json::Value::String(instructions);
        }
        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(to_responses_api_tools(tools));
            body["tool_choice"] = serde_json::json!("auto");
        }

        debug!(
            model = %self.model,
            messages_count = messages.len(),
            tools_count = tools.len(),
            "openai complete_responses request"
        );
        trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "openai responses request body");

        let url = self.responses_sse_url();
        let http_resp = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = http_resp.status();
        if !status.is_success() {
            let retry_after_ms = retry_after_ms_from_headers(http_resp.headers());
            let body_text = http_resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                with_retry_after_marker(
                    format!("Responses API error HTTP {status}: {body_text}"),
                    retry_after_ms,
                )
            );
        }

        // Collect SSE events into text + tool calls.
        let mut text_buf = String::new();
        let mut fn_call_ids: Vec<String> = Vec::new();
        let mut fn_call_names: Vec<String> = Vec::new();
        let mut fn_call_args: Vec<String> = Vec::new();
        let mut input_tokens: u32 = 0;
        let mut output_tokens: u32 = 0;
        let mut cache_read_tokens: u32 = 0;
        let cache_write_tokens: u32 = 0;

        let full_body = http_resp.text().await.unwrap_or_default();
        for line in full_body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }

            let Ok(evt) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };

            match evt["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    if let Some(delta) = evt["delta"].as_str() {
                        text_buf.push_str(delta);
                    }
                },
                "response.output_item.added" => {
                    if evt["item"]["type"].as_str() == Some("function_call") {
                        fn_call_ids.push(evt["item"]["call_id"].as_str().unwrap_or("").to_string());
                        fn_call_names.push(evt["item"]["name"].as_str().unwrap_or("").to_string());
                        fn_call_args.push(String::new());
                    }
                },
                "response.function_call_arguments.delta" => {
                    if let Some(delta) = evt["delta"].as_str()
                        && let Some(last) = fn_call_args.last_mut()
                    {
                        last.push_str(delta);
                    }
                },
                "response.completed" => {
                    if let Some(u) = evt["response"]["usage"].as_object() {
                        input_tokens =
                            u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        output_tokens =
                            u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                        cache_read_tokens = u
                            .get("input_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                    }
                },
                "error" | "response.failed" => {
                    let msg = evt["error"]["message"]
                        .as_str()
                        .or_else(|| evt["response"]["error"]["message"].as_str())
                        .or_else(|| evt["message"].as_str())
                        .unwrap_or("unknown error");
                    anyhow::bail!("Responses API error: {msg}");
                },
                _ => {},
            }
        }

        let text = if text_buf.is_empty() {
            None
        } else {
            Some(text_buf)
        };

        let tool_calls: Vec<moltis_agents::model::ToolCall> = fn_call_ids
            .into_iter()
            .zip(fn_call_names)
            .zip(fn_call_args)
            .filter_map(|((id, name), args)| {
                let arguments: serde_json::Value = serde_json::from_str(&args)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                if name.is_empty() {
                    return None;
                }
                Some(moltis_agents::model::ToolCall {
                    id,
                    name,
                    arguments,
                })
            })
            .collect();

        Ok(CompletionResponse {
            text,
            tool_calls,
            usage: Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            },
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn reasoning_effort(&self) -> Option<moltis_agents::model::ReasoningEffort> {
        self.reasoning_effort
    }

    fn with_reasoning_effort(
        self: std::sync::Arc<Self>,
        effort: moltis_agents::model::ReasoningEffort,
    ) -> Option<std::sync::Arc<dyn LlmProvider>> {
        Some(std::sync::Arc::new(Self {
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            provider_name: self.provider_name.clone(),
            client: self.client,
            stream_transport: self.stream_transport,
            metadata_cache: tokio::sync::OnceCell::new(),
            tool_mode_override: self.tool_mode_override,
            reasoning_effort: Some(effort),
            wire_api: self.wire_api,
            cache_retention: self.cache_retention,
        }))
    }

    fn id(&self) -> &str {
        &self.model
    }

    fn supports_tools(&self) -> bool {
        match self.tool_mode_override {
            Some(moltis_config::ToolMode::Native) => true,
            Some(moltis_config::ToolMode::Text | moltis_config::ToolMode::Off) => false,
            Some(moltis_config::ToolMode::Auto) | None => supports_tools_for_model(&self.model),
        }
    }

    fn tool_mode(&self) -> Option<moltis_config::ToolMode> {
        self.tool_mode_override
    }

    fn context_window(&self) -> u32 {
        context_window_for_model(&self.model)
    }

    fn supports_vision(&self) -> bool {
        supports_vision_for_model(&self.model)
    }

    async fn model_metadata(&self) -> anyhow::Result<ModelMetadata> {
        let meta = self
            .metadata_cache
            .get_or_try_init(|| async {
                let url = format!("{}/models/{}", self.base_url, self.model);
                debug!(url = %url, model = %self.model, "fetching model metadata");

                let resp = self
                    .client
                    .get(&url)
                    .header(
                        "Authorization",
                        format!("Bearer {}", self.api_key.expose_secret()),
                    )
                    .send()
                    .await?;

                if !resp.status().is_success() {
                    anyhow::bail!(
                        "model metadata API returned HTTP {}",
                        resp.status().as_u16()
                    );
                }

                let body: serde_json::Value = resp.json().await?;

                // OpenAI uses "context_window", some compat providers use "context_length".
                let context_length = body
                    .get("context_window")
                    .or_else(|| body.get("context_length"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .unwrap_or_else(|| self.context_window());

                Ok(ModelMetadata {
                    id: body
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&self.model)
                        .to_string(),
                    context_length,
                })
            })
            .await?;
        Ok(meta.clone())
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
    ) -> anyhow::Result<CompletionResponse> {
        if matches!(self.wire_api, WireApi::Responses) {
            return self.complete_responses(messages, tools).await;
        }

        let mut openai_messages = self.serialize_messages_for_request(messages);
        self.apply_openrouter_cache_control(&mut openai_messages);
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": openai_messages,
        });
        self.apply_system_prompt_rewrite(&mut body);

        if !tools.is_empty() {
            body["tools"] = serde_json::Value::Array(to_openai_tools(tools));
        }

        self.apply_reasoning_effort_chat(&mut body);

        debug!(
            model = %self.model,
            messages_count = messages.len(),
            tools_count = tools.len(),
            reasoning_effort = ?self.reasoning_effort,
            "openai complete request"
        );
        trace!(body = %serde_json::to_string(&body).unwrap_or_default(), "openai request body");

        let http_resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header(
                "Authorization",
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = http_resp.status();
        if !status.is_success() {
            let retry_after_ms = retry_after_ms_from_headers(http_resp.headers());
            let body_text = http_resp.text().await.unwrap_or_default();
            if should_warn_on_api_error(status, &body_text) {
                warn!(
                    status = %status,
                    model = %self.model,
                    provider = %self.provider_name,
                    body = %body_text,
                    "openai API error"
                );
            } else {
                debug!(
                    status = %status,
                    model = %self.model,
                    provider = %self.provider_name,
                    "openai model unsupported for chat/completions endpoint"
                );
            }
            anyhow::bail!(
                "{}",
                with_retry_after_marker(
                    format!("OpenAI API error HTTP {status}: {body_text}"),
                    retry_after_ms,
                )
            );
        }

        let resp = http_resp.json::<serde_json::Value>().await?;
        trace!(response = %resp, "openai raw response");

        let message = &resp["choices"][0]["message"];

        let text = message["content"].as_str().and_then(|s| {
            let (visible, _thinking) = strip_think_tags(s);
            if visible.is_empty() {
                None
            } else {
                Some(visible)
            }
        });
        let tool_calls = parse_tool_calls(message);

        let usage = parse_openai_compat_usage_from_payload(&resp).unwrap_or_default();

        Ok(CompletionResponse {
            text,
            tool_calls,
            usage,
        })
    }

    #[allow(clippy::collapsible_if)]
    fn stream(
        &self,
        messages: Vec<ChatMessage>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        self.stream_with_tools(messages, vec![])
    }

    async fn probe(&self) -> anyhow::Result<()> {
        match self.wire_api {
            WireApi::Responses => self.probe_responses().await,
            WireApi::ChatCompletions => self.probe_chat_completions().await,
        }
    }

    #[allow(clippy::collapsible_if)]
    fn stream_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
        match (self.wire_api, self.stream_transport) {
            (WireApi::Responses, ProviderStreamTransport::Sse) => {
                self.stream_responses_sse(messages, tools)
            },
            (WireApi::Responses, _) => {
                // WebSocket / Auto both go through the WS path which already
                // uses the responses format.
                self.stream_with_tools_websocket(
                    messages,
                    tools,
                    matches!(self.stream_transport, ProviderStreamTransport::Auto),
                )
            },
            (WireApi::ChatCompletions, ProviderStreamTransport::Sse) => {
                self.stream_with_tools_sse(messages, tools)
            },
            (WireApi::ChatCompletions, ProviderStreamTransport::Websocket) => {
                self.stream_with_tools_websocket(messages, tools, false)
            },
            (WireApi::ChatCompletions, ProviderStreamTransport::Auto) => {
                self.stream_with_tools_websocket(messages, tools, true)
            },
        }
    }
}
