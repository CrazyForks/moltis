//! Compaction strategy dispatcher.
//!
//! Routes a session history through the [`CompactionMode`] selected in
//! `chat.compaction`. Each strategy builds and returns the replacement
//! history; call sites handle storage and broadcast.
//!
//! See `docs/src/compaction.md` for the full mode comparison and trade-off
//! guidance, and the module rustdoc on [`moltis_config::CompactionMode`] for
//! per-variant semantics.

use {
    moltis_agents::model::LlmProvider,
    moltis_config::{CompactionConfig, CompactionMode},
    moltis_sessions::{MessageContent, PersistedMessage},
    serde_json::Value,
    thiserror::Error,
    tracing::info,
};
#[cfg(feature = "llm-compaction")]
use {
    moltis_agents::model::{ChatMessage, StreamEvent, values_to_chat_messages},
    tokio_stream::StreamExt,
};

/// Errors surfaced by [`run_compaction`].
///
/// Several variants are gated on the `llm-compaction` cargo feature; when
/// the feature is off the LLM-backed strategies aren't compiled in, so
/// their dedicated error variants become dead code.
#[derive(Debug, Error)]
pub(crate) enum CompactionRunError {
    /// History was empty — nothing to compact.
    #[error("nothing to compact")]
    EmptyHistory,
    /// The strategy produced no summary text.
    #[error("compact produced empty summary")]
    EmptySummary,
    /// A mode that requires an LLM provider was selected but none was passed.
    #[cfg(feature = "llm-compaction")]
    #[error("compaction mode '{mode}' requires an LLM provider to be available for the session")]
    ProviderRequired { mode: &'static str },
    /// The user selected a mode whose strategy isn't implemented yet.
    #[error(
        "compaction mode '{mode}' is not yet implemented (tracked by beads issue {issue}); \
         set chat.compaction.mode to 'deterministic' or 'llm_replace' in the meantime"
    )]
    NotYetImplemented {
        mode: &'static str,
        issue: &'static str,
    },
    /// The user selected a mode that requires a cargo feature that isn't enabled.
    #[cfg(not(feature = "llm-compaction"))]
    #[error("compaction mode '{mode}' requires the 'llm-compaction' cargo feature to be enabled")]
    FeatureDisabled { mode: &'static str },
    /// The LLM streaming summary call failed.
    #[cfg(feature = "llm-compaction")]
    #[error("compact summarization failed: {0}")]
    LlmFailed(String),
}

/// Run the compaction strategy selected by `config` against `history`.
///
/// Returns the replacement history vec. Call sites are responsible for
/// writing the result back to the session store.
///
/// `provider` is only consulted by LLM-backed modes; pass `None` when no
/// provider has been resolved for the session. LLM modes return
/// [`CompactionRunError::ProviderRequired`] when called without one.
pub(crate) async fn run_compaction(
    history: &[Value],
    config: &CompactionConfig,
    provider: Option<&dyn LlmProvider>,
) -> Result<Vec<Value>, CompactionRunError> {
    if history.is_empty() {
        return Err(CompactionRunError::EmptyHistory);
    }

    match config.mode {
        CompactionMode::Deterministic => deterministic_strategy(history),
        CompactionMode::LlmReplace => {
            #[cfg(feature = "llm-compaction")]
            {
                let provider = provider.ok_or(CompactionRunError::ProviderRequired {
                    mode: "llm_replace",
                })?;
                llm_replace_strategy(history, config, provider).await
            }
            #[cfg(not(feature = "llm-compaction"))]
            {
                let _ = (config, provider);
                Err(CompactionRunError::FeatureDisabled {
                    mode: "llm_replace",
                })
            }
        },
        CompactionMode::RecencyPreserving => Err(CompactionRunError::NotYetImplemented {
            mode: "recency_preserving",
            issue: "moltis-h0c",
        }),
        CompactionMode::Structured => Err(CompactionRunError::NotYetImplemented {
            mode: "structured",
            issue: "moltis-aff",
        }),
    }
}

/// `CompactionMode::Deterministic` strategy — current PR #653 behaviour.
///
/// Runs the structured-extraction helpers in `crate::compaction`, compresses
/// the summary to fit the budget, and wraps it in a single user message.
fn deterministic_strategy(history: &[Value]) -> Result<Vec<Value>, CompactionRunError> {
    let merged = crate::compaction::compute_compaction_summary(history)
        .ok_or(CompactionRunError::EmptySummary)?;
    let summary = crate::compaction::compress_summary(&merged);
    if summary.is_empty() {
        return Err(CompactionRunError::EmptySummary);
    }

    info!(
        messages = history.len(),
        "chat.compact: deterministic summary"
    );

    Ok(vec![build_summary_message(
        &crate::compaction::get_compact_continuation_message(&summary, false),
    )])
}

/// `CompactionMode::LlmReplace` strategy — pre-PR #653 behaviour.
///
/// Streams a plain-text summary from the provider, then replaces the entire
/// history with a single user message containing it. Preserved for users who
/// explicitly want the old behaviour or need maximum token reduction.
#[cfg(feature = "llm-compaction")]
async fn llm_replace_strategy(
    history: &[Value],
    config: &CompactionConfig,
    provider: &dyn LlmProvider,
) -> Result<Vec<Value>, CompactionRunError> {
    // Build a structured prompt around the history so role boundaries are
    // maintained via the API's message structure. This prevents prompt
    // injection where user content could mimic role prefixes if we
    // concatenated everything into a single text blob.
    let mut summary_messages = vec![ChatMessage::system(
        "You are a conversation summarizer. The messages that follow are a \
         conversation you must summarize. Preserve all key facts, decisions, \
         and context. After the conversation, you will receive a final \
         instruction.",
    )];
    summary_messages.extend(values_to_chat_messages(history));
    summary_messages.push(ChatMessage::user(
        "Summarize the conversation above into a concise form. Output only \
         the summary, no preamble.",
    ));

    let mut stream = provider.stream(summary_messages);
    let mut summary = String::new();
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::Delta(delta) => summary.push_str(&delta),
            StreamEvent::Done(_) => break,
            StreamEvent::Error(e) => {
                return Err(CompactionRunError::LlmFailed(e.to_string()));
            },
            // Tool events aren't expected on a summary stream; drop them.
            StreamEvent::ToolCallStart { .. }
            | StreamEvent::ToolCallArgumentsDelta { .. }
            | StreamEvent::ToolCallComplete { .. }
            // Provider raw payloads are debug metadata, not summary text.
            | StreamEvent::ProviderRaw(_)
            // Ignore provider reasoning blocks; the summary body should only
            // include final answer text.
            | StreamEvent::ReasoningDelta(_) => {},
        }
    }

    // `config.summary_model` / `max_summary_tokens` aren't wired yet —
    // tracked by beads issue moltis-8me. Silence unused-field lint without
    // leaking that into the public API.
    let _ = config;

    if summary.is_empty() {
        return Err(CompactionRunError::EmptySummary);
    }

    info!(
        messages = history.len(),
        chars = summary.len(),
        "chat.compact: llm_replace summary"
    );

    Ok(vec![build_summary_message(&summary)])
}

/// Wrap a summary string in a `PersistedMessage::User` ready for `replace_history`.
///
/// Using the `user` role (not `assistant`) avoids breaking strict providers
/// (e.g. llama.cpp) that require every assistant message to follow a user
/// message, and keeps the summary in the conversation turn array for
/// providers using the Responses API (which promote system messages to
/// instructions and drop them from turns).
fn build_summary_message(body: &str) -> Value {
    let msg = PersistedMessage::User {
        content: MessageContent::Text(format!("[Conversation Summary]\n\n{body}")),
        created_at: Some(crate::now_ms()),
        audio: None,
        channel: None,
        seq: None,
        run_id: None,
    };
    msg.to_value()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use {super::*, serde_json::json};

    fn sample_history() -> Vec<Value> {
        vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi there"}),
            json!({"role": "user", "content": "what is 2+2"}),
            json!({"role": "assistant", "content": "4"}),
        ]
    }

    #[tokio::test]
    async fn empty_history_returns_empty_history_error() {
        let config = CompactionConfig::default();
        let err = run_compaction(&[], &config, None).await.unwrap_err();
        assert!(matches!(err, CompactionRunError::EmptyHistory));
    }

    #[tokio::test]
    async fn deterministic_mode_returns_single_summary_message() {
        let history = sample_history();
        let config = CompactionConfig::default();
        let result = run_compaction(&history, &config, None).await.unwrap();
        assert_eq!(
            result.len(),
            1,
            "deterministic mode replaces history with one message"
        );
        let text = result[0]
            .get("content")
            .and_then(Value::as_str)
            .expect("summary has string content");
        assert!(
            text.starts_with("[Conversation Summary]\n\n"),
            "summary is wrapped in the expected preamble, got: {text}"
        );
    }

    #[tokio::test]
    async fn recency_preserving_mode_is_not_yet_implemented() {
        let history = sample_history();
        let config = CompactionConfig {
            mode: CompactionMode::RecencyPreserving,
            ..Default::default()
        };
        let err = run_compaction(&history, &config, None).await.unwrap_err();
        match err {
            CompactionRunError::NotYetImplemented { mode, issue } => {
                assert_eq!(mode, "recency_preserving");
                assert_eq!(issue, "moltis-h0c");
            },
            other => panic!("expected NotYetImplemented, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn structured_mode_is_not_yet_implemented() {
        let history = sample_history();
        let config = CompactionConfig {
            mode: CompactionMode::Structured,
            ..Default::default()
        };
        let err = run_compaction(&history, &config, None).await.unwrap_err();
        match err {
            CompactionRunError::NotYetImplemented { mode, issue } => {
                assert_eq!(mode, "structured");
                assert_eq!(issue, "moltis-aff");
            },
            other => panic!("expected NotYetImplemented, got {other:?}"),
        }
    }

    #[cfg(feature = "llm-compaction")]
    #[tokio::test]
    async fn llm_replace_mode_without_provider_returns_provider_required() {
        let history = sample_history();
        let config = CompactionConfig {
            mode: CompactionMode::LlmReplace,
            ..Default::default()
        };
        let err = run_compaction(&history, &config, None).await.unwrap_err();
        match err {
            CompactionRunError::ProviderRequired { mode } => {
                assert_eq!(mode, "llm_replace");
            },
            other => panic!("expected ProviderRequired, got {other:?}"),
        }
    }

    #[cfg(not(feature = "llm-compaction"))]
    #[tokio::test]
    async fn llm_replace_mode_returns_feature_disabled_when_feature_off() {
        let history = sample_history();
        let config = CompactionConfig {
            mode: CompactionMode::LlmReplace,
            ..Default::default()
        };
        let err = run_compaction(&history, &config, None).await.unwrap_err();
        match err {
            CompactionRunError::FeatureDisabled { mode } => {
                assert_eq!(mode, "llm_replace");
            },
            other => panic!("expected FeatureDisabled, got {other:?}"),
        }
    }

    #[cfg(feature = "llm-compaction")]
    #[tokio::test]
    async fn llm_replace_mode_with_stub_provider_returns_single_message() {
        use {
            anyhow::Result,
            async_trait::async_trait,
            futures::Stream,
            moltis_agents::model::{CompletionResponse, Usage},
            std::pin::Pin,
        };

        struct StubProvider;

        #[async_trait]
        impl LlmProvider for StubProvider {
            fn name(&self) -> &str {
                "stub"
            }

            fn id(&self) -> &str {
                "stub::compaction"
            }

            async fn complete(
                &self,
                _messages: &[ChatMessage],
                _tools: &[Value],
            ) -> Result<CompletionResponse> {
                anyhow::bail!("stub does not implement complete")
            }

            fn stream(
                &self,
                _messages: Vec<ChatMessage>,
            ) -> Pin<Box<dyn Stream<Item = StreamEvent> + Send + '_>> {
                Box::pin(tokio_stream::iter(vec![
                    StreamEvent::Delta("stubbed summary body".into()),
                    StreamEvent::Done(Usage::default()),
                ]))
            }
        }

        let history = sample_history();
        let config = CompactionConfig {
            mode: CompactionMode::LlmReplace,
            ..Default::default()
        };
        let provider = StubProvider;
        let result = run_compaction(&history, &config, Some(&provider))
            .await
            .expect("llm_replace succeeds with stub provider");
        assert_eq!(result.len(), 1);
        let text = result[0]
            .get("content")
            .and_then(Value::as_str)
            .expect("summary content");
        assert!(text.contains("stubbed summary body"), "got: {text}");
    }
}
