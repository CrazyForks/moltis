#![allow(unused_imports)]

pub mod provider;
#[allow(unused_imports)]
pub use crate::DiscoveredModel;
#[allow(unused_imports)]
pub(crate) use crate::http::{retry_after_ms_from_headers, with_retry_after_marker};
#[allow(unused_imports)]
pub(crate) use crate::ollama::normalize_ollama_api_base_url;
#[allow(unused_imports)]
pub use crate::openai_compat;
#[allow(unused_imports)]
pub use crate::ws_pool;
#[allow(unused_imports)]
pub(crate) use crate::{
    context_window_for_model, raw_model_id, supports_tools_for_model, supports_vision_for_model,
};
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::mpsc,
    time::Duration,
};

use {
    async_trait::async_trait,
    futures::{SinkExt, StreamExt},
    moltis_config::schema::{ProviderStreamTransport, WireApi},
    secrecy::ExposeSecret,
    tokio_stream::Stream,
    tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

use tracing::{debug, trace, warn};

use {
    super::openai_compat::{
        ResponsesStreamState, SseLineResult, StreamingToolState, finalize_responses_stream,
        finalize_stream, parse_openai_compat_usage, parse_openai_compat_usage_from_payload,
        parse_tool_calls, process_openai_sse_line, process_responses_sse_line,
        responses_output_index, split_responses_instructions_and_input, strip_think_tags,
        to_openai_tools, to_responses_api_tools,
    },
    moltis_agents::model::{
        ChatMessage, CompletionResponse, LlmProvider, ModelMetadata, StreamEvent, Usage,
    },
};

pub struct OpenAiProvider {
    api_key: secrecy::Secret<String>,
    model: String,
    base_url: String,
    provider_name: String,
    client: &'static reqwest::Client,
    stream_transport: ProviderStreamTransport,
    wire_api: WireApi,
    metadata_cache: tokio::sync::OnceCell<ModelMetadata>,
    tool_mode_override: Option<moltis_config::ToolMode>,
    /// Optional reasoning effort level for o-series models.
    reasoning_effort: Option<moltis_agents::model::ReasoningEffort>,
    /// Prompt cache retention policy (used for OpenRouter Anthropic passthrough).
    cache_retention: moltis_config::CacheRetention,
}

const OPENAI_MODELS_ENDPOINT_PATH: &str = "/models";

#[derive(Clone, Copy)]
struct ModelCatalogEntry {
    id: &'static str,
    display_name: &'static str,
}

impl ModelCatalogEntry {
    const fn new(id: &'static str, display_name: &'static str) -> Self {
        Self { id, display_name }
    }
}

const DEFAULT_OPENAI_MODELS: &[ModelCatalogEntry] = &[
    ModelCatalogEntry::new("gpt-5.2", "GPT-5.2"),
    ModelCatalogEntry::new("gpt-5.2-chat-latest", "GPT-5.2 Chat Latest"),
    ModelCatalogEntry::new("gpt-5-mini", "GPT-5 Mini"),
];

#[must_use]
pub fn default_model_catalog() -> Vec<DiscoveredModel> {
    DEFAULT_OPENAI_MODELS
        .iter()
        .map(|entry| {
            DiscoveredModel::new(entry.id, entry.display_name)
                .with_recommended(is_recommended_openai_model(entry.id))
        })
        .collect()
}

fn title_case_chunk(chunk: &str) -> String {
    if chunk.is_empty() {
        return String::new();
    }
    let mut chars = chunk.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::new();
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
            out
        },
        None => String::new(),
    }
}

fn format_gpt_display_name(model_id: &str) -> String {
    let Some(rest) = model_id.strip_prefix("gpt-") else {
        return model_id.to_string();
    };
    let mut parts = rest.split('-');
    let Some(base) = parts.next() else {
        return "GPT".to_string();
    };
    let mut out = format!("GPT-{base}");
    for part in parts {
        out.push(' ');
        out.push_str(&title_case_chunk(part));
    }
    out
}

fn format_chatgpt_display_name(model_id: &str) -> String {
    let Some(rest) = model_id.strip_prefix("chatgpt-") else {
        return model_id.to_string();
    };
    let mut parts = rest.split('-');
    let Some(base) = parts.next() else {
        return "ChatGPT".to_string();
    };
    let mut out = format!("ChatGPT-{base}");
    for part in parts {
        out.push(' ');
        out.push_str(&title_case_chunk(part));
    }
    out
}

fn formatted_model_name(model_id: &str) -> String {
    if model_id.starts_with("gpt-") {
        return format_gpt_display_name(model_id);
    }
    if model_id.starts_with("chatgpt-") {
        return format_chatgpt_display_name(model_id);
    }
    model_id.to_string()
}

fn normalize_display_name(model_id: &str, display_name: Option<&str>) -> String {
    let normalized = display_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(model_id);
    if normalized == model_id {
        return formatted_model_name(model_id);
    }
    normalized.to_string()
}

fn is_likely_model_id(model_id: &str) -> bool {
    if model_id.is_empty() || model_id.len() > 160 {
        return false;
    }
    if model_id.chars().any(char::is_whitespace) {
        return false;
    }
    model_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/'))
}

/// Delegates to the shared [`super::is_chat_capable_model`] for filtering
/// non-chat models during discovery.
fn is_chat_capable_model(model_id: &str) -> bool {
    super::is_chat_capable_model(model_id)
}

fn parse_model_entry(entry: &serde_json::Value) -> Option<DiscoveredModel> {
    let obj = entry.as_object()?;
    let model_id = obj
        .get("id")
        .or_else(|| obj.get("slug"))
        .or_else(|| obj.get("model"))
        .and_then(serde_json::Value::as_str)?;

    if !is_likely_model_id(model_id) {
        return None;
    }

    let display_name = obj
        .get("display_name")
        .or_else(|| obj.get("displayName"))
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("title"))
        .and_then(serde_json::Value::as_str);

    let created_at = obj.get("created").and_then(serde_json::Value::as_i64);

    let recommended = is_recommended_openai_model(model_id);
    Some(
        DiscoveredModel::new(model_id, normalize_display_name(model_id, display_name))
            .with_created_at(created_at)
            .with_recommended(recommended),
    )
}

/// Known OpenAI flagship model IDs (latest generation, no date suffix).
/// These are the models most users care about.
fn is_recommended_openai_model(model_id: &str) -> bool {
    matches!(
        model_id,
        "gpt-5.4" | "gpt-5.4-mini" | "gpt-5.4-pro" | "o4-mini" | "o3"
    )
}

fn collect_candidate_arrays<'a>(
    value: &'a serde_json::Value,
    out: &mut Vec<&'a serde_json::Value>,
) {
    match value {
        serde_json::Value::Array(items) => out.extend(items),
        serde_json::Value::Object(map) => {
            for key in ["models", "data", "items", "results", "available"] {
                if let Some(nested) = map.get(key) {
                    collect_candidate_arrays(nested, out);
                }
            }
        },
        _ => {},
    }
}

fn parse_models_payload(value: &serde_json::Value) -> Vec<DiscoveredModel> {
    let mut candidates = Vec::new();
    collect_candidate_arrays(value, &mut candidates);

    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for entry in candidates {
        if let Some(model) = parse_model_entry(entry)
            && is_chat_capable_model(&model.id)
            && seen.insert(model.id.clone())
        {
            models.push(model);
        }
    }

    // Sort by created_at descending (newest first). Models without a
    // timestamp are placed after those with one, preserving relative order.
    models.sort_by(|a, b| match (a.created_at, b.created_at) {
        (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts), // newest first
        (Some(_), None) => std::cmp::Ordering::Less, // timestamp before no-timestamp
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    models
}

fn is_chat_endpoint_unsupported_model_error(body_text: &str) -> bool {
    let lower = body_text.to_ascii_lowercase();
    lower.contains("not a chat model")
        || lower.contains("does not support chat")
        || lower.contains("only supported in v1/responses")
        || lower.contains("not supported in the v1/chat/completions endpoint")
        || lower.contains("input content or output modality contain audio")
        || lower.contains("requires audio")
}

fn should_warn_on_api_error(status: reqwest::StatusCode, body_text: &str) -> bool {
    if is_chat_endpoint_unsupported_model_error(body_text) {
        return false;
    }
    !matches!(status.as_u16(), 404)
}

const OPENAI_MAX_TOOL_CALL_ID_LEN: usize = 40;

fn short_stable_hash(value: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn base_openai_tool_call_id(raw: &str) -> String {
    let mut cleaned: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if cleaned.is_empty() {
        cleaned = "call".to_string();
    }

    if cleaned.len() <= OPENAI_MAX_TOOL_CALL_ID_LEN {
        return cleaned;
    }

    let hash = short_stable_hash(raw);
    let keep = OPENAI_MAX_TOOL_CALL_ID_LEN.saturating_sub(hash.len() + 1);
    cleaned.truncate(keep);
    if cleaned.is_empty() {
        return format!("call-{hash}");
    }
    format!("{cleaned}-{hash}")
}

fn disambiguate_tool_call_id(base: &str, nonce: usize) -> String {
    let suffix = format!("-{nonce}");
    let keep = OPENAI_MAX_TOOL_CALL_ID_LEN.saturating_sub(suffix.len());

    let mut value = base.to_string();
    if value.len() > keep {
        value.truncate(keep);
    }
    if value.is_empty() {
        value = "call".to_string();
        if value.len() > keep {
            value.truncate(keep);
        }
    }
    format!("{value}{suffix}")
}

fn assign_openai_tool_call_id(
    raw: &str,
    remapped_tool_call_ids: &mut HashMap<String, String>,
    used_tool_call_ids: &mut HashSet<String>,
) -> String {
    if let Some(existing) = remapped_tool_call_ids.get(raw) {
        return existing.clone();
    }

    let base = base_openai_tool_call_id(raw);
    let mut candidate = base.clone();
    let mut nonce = 1usize;
    while used_tool_call_ids.contains(&candidate) {
        candidate = disambiguate_tool_call_id(&base, nonce);
        nonce = nonce.saturating_add(1);
    }

    used_tool_call_ids.insert(candidate.clone());
    remapped_tool_call_ids.insert(raw.to_string(), candidate.clone());
    candidate
}

fn models_endpoint(base_url: &str) -> String {
    format!(
        "{}{OPENAI_MODELS_ENDPOINT_PATH}",
        base_url.trim_end_matches('/')
    )
}

/// Resolve the output index from a Responses API WebSocket streaming event.
///
/// The Responses API includes `output_index` on most events. Falls back to
/// `item_index` / `index` for robustness, then to `fallback`.
pub async fn fetch_models_from_api(
    api_key: secrecy::Secret<String>,
    base_url: String,
) -> anyhow::Result<Vec<DiscoveredModel>> {
    let client = crate::shared_http_client();
    let response = client
        .get(models_endpoint(&base_url))
        .timeout(Duration::from_secs(15))
        .header(
            "Authorization",
            format!("Bearer {}", api_key.expose_secret()),
        )
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("openai models API error HTTP {status}");
    }
    let payload: serde_json::Value = serde_json::from_str(&body)?;
    let models = parse_models_payload(&payload);
    if models.is_empty() {
        anyhow::bail!("openai models API returned no models");
    }
    Ok(models)
}

/// Spawn model discovery in a background thread and return the receiver
/// immediately, without blocking. Call `.recv()` later to collect the result.
pub fn start_model_discovery(
    api_key: secrecy::Secret<String>,
    base_url: String,
) -> mpsc::Receiver<anyhow::Result<Vec<DiscoveredModel>>> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(fetch_models_from_api(api_key, base_url)));
        let _ = tx.send(result);
    });
    rx
}

fn fetch_models_blocking(
    api_key: secrecy::Secret<String>,
    base_url: String,
) -> anyhow::Result<Vec<DiscoveredModel>> {
    start_model_discovery(api_key, base_url)
        .recv()
        .map_err(|err| anyhow::anyhow!("openai model discovery worker failed: {err}"))?
}

pub fn live_models(
    api_key: &secrecy::Secret<String>,
    base_url: &str,
) -> anyhow::Result<Vec<DiscoveredModel>> {
    let models = fetch_models_blocking(api_key.clone(), base_url.to_string())?;
    debug!(model_count = models.len(), "loaded live models");
    Ok(models)
}

#[must_use]
pub fn available_models(api_key: &secrecy::Secret<String>, base_url: &str) -> Vec<DiscoveredModel> {
    let fallback = default_model_catalog();
    if cfg!(test) {
        return fallback;
    }

    let discovered = match live_models(api_key, base_url) {
        Ok(models) => models,
        Err(err) => {
            warn!(error = %err, base_url = %base_url, "failed to fetch openai models, using fallback catalog");
            return fallback;
        },
    };

    let merged = super::merge_discovered_with_fallback_catalog(discovered, fallback);
    debug!(model_count = merged.len(), "loaded openai models catalog");
    merged
}
