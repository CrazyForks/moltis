// ── Attribution ──────────────────────────────────────────────────────
// Deterministic compaction extraction adapted from claw-code (ultraworkers/claw-code).
// Original source: rust/crates/runtime/src/compact.rs
// License: MIT — Copyright (c) ultraworkers
// https://github.com/ultraworkers/claw-code

//! Deterministic conversation compaction — zero LLM calls.
//!
//! Extracts structured summaries from session history by inspecting JSON message
//! values directly. Produces consistent, auditable output for the same input.

use serde_json::Value;

const COMPACT_CONTINUATION_PREAMBLE: &str = "This session is being continued from a previous conversation that ran out of context. \
    The summary below covers the earlier portion of the conversation.\n\n";
const COMPACT_RECENT_MESSAGES_NOTE: &str = "Recent messages are preserved verbatim.";
const COMPACT_DIRECT_RESUME_INSTRUCTION: &str = "Continue the conversation from where it left off without asking the user any further \
    questions. Resume directly — do not acknowledge the summary, do not recap what was \
    happening, and do not preface with continuation text.";

/// Produce a structured summary string from a slice of JSON message values.
///
/// Extracts: message counts by role, tool names, key files, recent user requests,
/// pending work, current work, and a verbatim timeline.
/// Zero LLM calls — pure string/JSON inspection.
#[must_use]
pub fn summarize_messages(messages: &[Value]) -> String {
    let user_count = messages.iter().filter(|m| m["role"] == "user").count();
    let assistant_count = messages.iter().filter(|m| m["role"] == "assistant").count();
    let tool_count = messages
        .iter()
        .filter(|m| m["role"] == "tool" || m["role"] == "tool_result")
        .count();

    let mut tool_names: Vec<&str> = messages
        .iter()
        .flat_map(|m| {
            let mut names = Vec::new();
            // From assistant tool_calls
            if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    if let Some(name) = call
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                    {
                        names.push(name);
                    }
                }
            }
            // From tool_result messages
            if let Some(name) = m.get("tool_name").and_then(Value::as_str) {
                names.push(name);
            }
            // From tool messages (legacy)
            if let Some(name) = m.get("name").and_then(Value::as_str) {
                names.push(name);
            }
            names
        })
        .collect();
    tool_names.sort_unstable();
    tool_names.dedup();

    let mut lines = vec![
        "<summary>".to_string(),
        "Conversation summary:".to_string(),
        format!(
            "- Scope: {} earlier messages compacted (user={}, assistant={}, tool={}).",
            messages.len(),
            user_count,
            assistant_count,
            tool_count
        ),
    ];

    if !tool_names.is_empty() {
        lines.push(format!("- Tools mentioned: {}.", tool_names.join(", ")));
    }

    let recent_user_requests = collect_recent_role_summaries(messages, "user", 3);
    if !recent_user_requests.is_empty() {
        lines.push("- Recent user requests:".to_string());
        lines.extend(recent_user_requests.into_iter().map(|r| format!("  - {r}")));
    }

    let pending_work = infer_pending_work(messages);
    if !pending_work.is_empty() {
        lines.push("- Pending work:".to_string());
        lines.extend(pending_work.into_iter().map(|item| format!("  - {item}")));
    }

    let key_files = collect_key_files(messages);
    if !key_files.is_empty() {
        lines.push(format!("- Key files referenced: {}.", key_files.join(", ")));
    }

    if let Some(current_work) = infer_current_work(messages) {
        lines.push(format!("- Current work: {current_work}"));
    }

    lines.push("- Key timeline:".to_string());
    for message in messages {
        let role = message["role"].as_str().unwrap_or("unknown");
        let content = extract_content_preview(message);
        lines.push(format!("  - {role}: {content}"));
    }
    lines.push("</summary>".to_string());
    lines.join("\n")
}

/// Merge a previous compaction summary with a new one for re-compaction.
///
/// Preserves previous highlights, drops old timeline, adds new highlights + timeline.
#[must_use]
pub fn merge_compact_summaries(existing_summary: Option<&str>, new_summary: &str) -> String {
    let Some(existing_summary) = existing_summary else {
        return new_summary.to_string();
    };

    let previous_highlights = extract_summary_highlights(existing_summary);
    let new_formatted_summary = format_compact_summary(new_summary);
    let new_highlights = extract_summary_highlights(&new_formatted_summary);
    let new_timeline = extract_summary_timeline(&new_formatted_summary);

    let mut lines = vec!["<summary>".to_string(), "Conversation summary:".to_string()];

    if !previous_highlights.is_empty() {
        lines.push("- Previously compacted context:".to_string());
        lines.extend(
            previous_highlights
                .into_iter()
                .map(|line| format!("  {line}")),
        );
    }

    if !new_highlights.is_empty() {
        lines.push("- Newly compacted context:".to_string());
        lines.extend(new_highlights.into_iter().map(|line| format!("  {line}")));
    }

    if !new_timeline.is_empty() {
        lines.push("- Key timeline:".to_string());
        lines.extend(new_timeline.into_iter().map(|line| format!("  {line}")));
    }

    lines.push("</summary>".to_string());
    lines.join("\n")
}

/// Build the synthetic continuation message injected after compaction.
///
/// Three parts: preamble + formatted summary, recent-messages note, direct-resume instruction.
#[must_use]
pub fn get_compact_continuation_message(summary: &str, recent_messages_preserved: bool) -> String {
    let mut base = format!(
        "{COMPACT_CONTINUATION_PREAMBLE}{}",
        format_compact_summary(summary)
    );

    if recent_messages_preserved {
        base.push_str("\n\n");
        base.push_str(COMPACT_RECENT_MESSAGES_NOTE);
    }

    base.push('\n');
    base.push_str(COMPACT_DIRECT_RESUME_INSTRUCTION);

    base
}

/// Format the raw `<summary>...</summary>` block for user-facing display.
///
/// Strips analysis blocks, extracts summary content, collapses blank lines.
#[must_use]
pub fn format_compact_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    let formatted = if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        without_analysis.replace(
            &format!("<summary>{content}</summary>"),
            &format!("Summary:\n{}", content.trim()),
        )
    } else {
        without_analysis
    };

    collapse_blank_lines(&formatted).trim().to_string()
}

/// Detect whether history[0] is a previous compaction summary.
///
/// Returns `Some(summary_text)` if detected, `None` otherwise.
#[must_use]
pub fn extract_existing_compacted_summary(history: &[Value]) -> Option<String> {
    let first = history.first()?;
    let content = first.get("content").and_then(Value::as_str)?;
    let summary_text = content.strip_prefix("[Conversation Summary]\n\n")?;
    let summary = summary_text.trim();
    if summary.is_empty() {
        return None;
    }
    Some(summary.to_string())
}

// ── Private helpers ──────────────────────────────────────────────────

/// Single content block preview (160 char truncation).
fn summarize_block(content: &str) -> String {
    truncate_summary(content, 160)
}

/// Extract a text preview from a JSON message value.
fn extract_content_preview(message: &Value) -> String {
    let mut parts = Vec::new();

    // Text content
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        parts.push(summarize_block(text));
    } else if let Some(blocks) = message.get("content").and_then(Value::as_array) {
        for block in blocks {
            if block["type"] == "text"
                && let Some(text) = block.get("text").and_then(Value::as_str)
            {
                parts.push(summarize_block(text));
            }
        }
    }

    // Tool calls
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let args = call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            parts.push(summarize_block(&format!("tool_use {name}({args})")));
        }
    }

    // Tool result
    if message["role"] == "tool" || message["role"] == "tool_result" {
        let tool_name = message
            .get("tool_name")
            .or_else(|| message.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let is_error = message.get("error").and_then(Value::as_str).is_some()
            || message
                .get("success")
                .and_then(Value::as_bool)
                .is_some_and(|s| !s);
        let result_text = message.get("content").and_then(Value::as_str).unwrap_or("");
        let prefix = if is_error {
            "error "
        } else {
            ""
        };
        parts.push(summarize_block(&format!(
            "tool_result {tool_name}: {prefix}{result_text}"
        )));
    }

    if parts.is_empty() {
        "(empty)".to_string()
    } else {
        parts.join(" | ")
    }
}

/// Collect recent text previews for messages matching a given role.
fn collect_recent_role_summaries(messages: &[Value], role: &str, limit: usize) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m["role"] == role)
        .rev()
        .filter_map(first_text_block)
        .take(limit)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Keyword-based inference of pending work items.
fn infer_pending_work(messages: &[Value]) -> Vec<String> {
    const KEYWORDS: &[&str] = &["todo", "next", "pending", "follow up", "remaining"];

    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .filter(|text| {
            let lowered = text.to_ascii_lowercase();
            KEYWORDS.iter().any(|kw| lowered.contains(kw))
        })
        .take(3)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Extract file paths with interesting extensions from message content.
fn collect_key_files(messages: &[Value]) -> Vec<String> {
    let mut files: Vec<String> = messages
        .iter()
        .flat_map(|m| {
            let mut texts: Vec<&str> = Vec::new();
            if let Some(text) = m.get("content").and_then(Value::as_str) {
                texts.push(text);
            } else if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                for block in blocks {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        texts.push(text);
                    }
                }
            }
            if let Some(args) = m
                .get("tool_calls")
                .and_then(Value::as_array)
                .and_then(|calls| {
                    calls
                        .first()
                        .and_then(|c| c.get("function"))
                        .and_then(|f| f.get("arguments"))
                        .and_then(Value::as_str)
                })
            {
                texts.push(args);
            }
            texts
                .into_iter()
                .flat_map(extract_file_candidates)
                .collect::<Vec<_>>()
        })
        .collect();
    files.sort();
    files.dedup();
    files.into_iter().take(8).collect()
}

/// Infer the most recent non-empty assistant text as "current work".
fn infer_current_work(messages: &[Value]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter(|m| m["role"] == "assistant")
        .filter_map(first_text_block)
        .find(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, 200))
}

/// Extract the first non-empty text from a JSON message value.
fn first_text_block(message: &Value) -> Option<&str> {
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    if let Some(blocks) = message.get("content").and_then(Value::as_array) {
        for block in blocks {
            if block["type"] == "text"
                && let Some(text) = block.get("text").and_then(Value::as_str)
            {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }
    None
}

/// Extract file path candidates from content using whitespace splitting.
fn extract_file_candidates(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter_map(|token| {
            let candidate = token.trim_matches(|c: char| {
                matches!(c, ',' | '.' | ':' | ';' | ')' | '(' | '"' | '\'' | '`')
            });
            if candidate.contains('/') && has_interesting_extension(candidate) {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Check if a path has an interesting source code extension.
fn has_interesting_extension(candidate: &str) -> bool {
    std::path::Path::new(candidate)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["rs", "ts", "tsx", "js", "json", "md"]
                .iter()
                .any(|expected| ext.eq_ignore_ascii_case(expected))
        })
}

/// Truncate content to max_chars, appending ellipsis if truncated.
fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated: String = content.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

/// Rough token estimate: content length / 4 + 1.
#[allow(dead_code)]
fn estimate_message_tokens(message: &Value) -> usize {
    let content_len = if let Some(text) = message.get("content").and_then(Value::as_str) {
        text.len()
    } else if let Some(blocks) = message.get("content").and_then(Value::as_array) {
        blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .map(|t| t.len())
            .sum()
    } else {
        0
    };

    let tool_len: usize = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|c| {
                    c.get("function")
                        .map(|f| {
                            f.get("name").and_then(Value::as_str).map_or(0, |n| n.len())
                                + f.get("arguments")
                                    .and_then(Value::as_str)
                                    .map_or(0, |a| a.len())
                        })
                        .unwrap_or(0)
                })
                .sum::<usize>()
        })
        .unwrap_or(0);

    (content_len + tool_len) / 4 + 1
}

/// Extract bullet lines (starting with `-`) as highlights.
fn extract_summary_highlights(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed == "Summary:" || trimmed == "Conversation summary:" {
            continue;
        }
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if in_timeline {
            continue;
        }
        lines.push(trimmed.to_string());
    }

    lines
}

/// Extract timeline lines from the "Key timeline:" section.
fn extract_summary_timeline(summary: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_timeline = false;

    for line in format_compact_summary(summary).lines() {
        let trimmed = line.trim_end();
        if trimmed == "- Key timeline:" {
            in_timeline = true;
            continue;
        }
        if !in_timeline {
            continue;
        }
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }

    lines
}

/// Collapse consecutive blank lines into a single newline.
fn collapse_blank_lines(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut last_blank = false;
    for line in content.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && last_blank {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        last_blank = is_blank;
    }
    result
}

/// Extract the content between `<tag>...</tag>` markers.
fn extract_tag_block(content: &str, tag: &str) -> Option<String> {
    let start_marker = format!("<{tag}>");
    let end_marker = format!("</{tag}>");
    let start_idx = content.find(&start_marker)? + start_marker.len();
    let end_idx = content[start_idx..].find(&end_marker)? + start_idx;
    Some(content[start_idx..end_idx].to_string())
}

/// Remove a `<tag>...</tag>` block from content.
fn strip_tag_block(content: &str, tag: &str) -> String {
    let start_marker = format!("<{tag}>");
    let end_marker = format!("</{tag}>");
    if let (Some(start_idx), Some(end_idx_rel)) =
        (content.find(&start_marker), content.find(&end_marker))
    {
        let end_idx = end_idx_rel + end_marker.len();
        let mut stripped = String::with_capacity(content.len());
        stripped.push_str(&content[..start_idx]);
        stripped.push_str(&content[end_idx..]);
        stripped
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use {super::*, serde_json::json};

    fn make_user(text: &str) -> Value {
        json!({
            "role": "user",
            "content": text
        })
    }

    fn make_assistant(text: &str) -> Value {
        json!({
            "role": "assistant",
            "content": text
        })
    }

    fn make_assistant_with_tools(text: &str, tool_names: &[&str]) -> Value {
        let calls: Vec<Value> = tool_names
            .iter()
            .map(|name| {
                json!({
                    "id": format!("call_{name}"),
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": "{}"
                    }
                })
            })
            .collect();
        json!({
            "role": "assistant",
            "content": text,
            "tool_calls": calls
        })
    }

    fn make_tool_result(tool_name: &str, content: &str, success: bool) -> Value {
        json!({
            "role": "tool_result",
            "tool_name": tool_name,
            "content": content,
            "success": success
        })
    }

    // ── summarize_messages ──────────────────────────────────────────

    #[test]
    fn summarize_messages_basic() {
        let messages = vec![
            make_user("hello"),
            make_assistant("hi there"),
            make_user("how are you"),
            make_assistant("doing well"),
        ];
        let summary = summarize_messages(&messages);
        assert!(summary.contains("<summary>"));
        assert!(summary.contains("</summary>"));
        assert!(summary.contains("user=2"));
        assert!(summary.contains("assistant=2"));
        assert!(summary.contains("tool=0"));
        assert!(summary.contains("Scope: 4 earlier messages"));
    }

    #[test]
    fn summarize_messages_with_tools() {
        let messages = vec![
            make_user("run a search"),
            make_assistant_with_tools("searching", &["search", "read_file"]),
            make_tool_result("search", "found 5 files", true),
            make_tool_result("read_file", "file contents", true),
        ];
        let summary = summarize_messages(&messages);
        assert!(summary.contains("Tools mentioned: read_file, search"));
        assert!(summary.contains("tool=2"));
    }

    #[test]
    fn summarize_messages_key_files() {
        let messages = vec![make_user(
            "Update crates/chat/src/compaction.rs and src/main.rs next.",
        )];
        let summary = summarize_messages(&messages);
        assert!(summary.contains("crates/chat/src/compaction.rs"));
        assert!(summary.contains("src/main.rs"));
    }

    #[test]
    fn summarize_messages_pending_work() {
        let messages = vec![
            make_user("do something"),
            make_assistant("Next: update the tests and follow up on remaining items."),
        ];
        let summary = summarize_messages(&messages);
        assert!(summary.contains("Pending work:"));
        assert!(summary.contains("Next: update the tests"));
    }

    #[test]
    fn summarize_messages_empty() {
        let summary = summarize_messages(&[]);
        assert!(summary.contains("user=0"));
        assert!(summary.contains("assistant=0"));
    }

    // ── merge_compact_summaries ──────────────────────────────────────

    #[test]
    fn merge_compact_summaries_first_compaction() {
        let new = "<summary>Conversation summary:\n- Scope: 4 messages.\n- Key timeline:\n  - user: hello\n</summary>";
        let result = merge_compact_summaries(None, new);
        assert_eq!(result, new);
    }

    #[test]
    fn merge_compact_summaries_recompaction() {
        let existing = "<summary>Conversation summary:\n- Scope: 2 messages.\n- Key files: src/main.rs.\n- Key timeline:\n  - user: old\n</summary>";
        let new = "<summary>Conversation summary:\n- Scope: 3 messages.\n- Key files: lib.rs.\n- Key timeline:\n  - user: new\n</summary>";

        let merged = merge_compact_summaries(Some(existing), new);
        assert!(merged.contains("Previously compacted context:"));
        assert!(merged.contains("Newly compacted context:"));
        assert!(merged.contains("Key files: src/main.rs"));
        assert!(merged.contains("Key files: lib.rs"));
        // Old timeline should be dropped, new timeline kept
        assert!(merged.contains("- user: new"));
    }

    // ── extract_existing_compacted_summary ───────────────────────────

    #[test]
    fn extract_existing_compacted_summary_detected() {
        let history = vec![json!({
            "role": "user",
            "content": "[Conversation Summary]\n\nSome summary text here"
        })];
        let result = extract_existing_compacted_summary(&history);
        assert_eq!(result, Some("Some summary text here".to_string()));
    }

    #[test]
    fn extract_existing_compacted_summary_not_found() {
        let history = vec![make_user("normal message")];
        let result = extract_existing_compacted_summary(&history);
        assert_eq!(result, None);
    }

    #[test]
    fn extract_existing_compacted_summary_empty_history() {
        let result: Option<String> = extract_existing_compacted_summary(&[]);
        assert_eq!(result, None);
    }

    // ── get_compact_continuation_message ─────────────────────────────

    #[test]
    fn get_compact_continuation_message_full() {
        let summary = "<summary>Test summary</summary>";
        let msg = get_compact_continuation_message(summary, true);
        assert!(msg.contains("continued from a previous conversation"));
        assert!(msg.contains("Summary:"));
        assert!(msg.contains("Test summary"));
        assert!(msg.contains("Recent messages are preserved verbatim"));
        assert!(msg.contains("Continue the conversation from where it left off"));
    }

    #[test]
    fn get_compact_continuation_message_no_recent() {
        let summary = "<summary>Test</summary>";
        let msg = get_compact_continuation_message(summary, false);
        assert!(!msg.contains("Recent messages are preserved"));
        assert!(msg.contains("Continue the conversation"));
    }

    // ── format_compact_summary ───────────────────────────────────────

    #[test]
    fn format_compact_summary_extracts_tag() {
        let raw = "<analysis>scratch</analysis>\n<summary>Kept work</summary>";
        let formatted = format_compact_summary(raw);
        assert_eq!(formatted, "Summary:\nKept work");
    }

    #[test]
    fn format_compact_summary_no_tags() {
        let raw = "Just plain text summary";
        let formatted = format_compact_summary(raw);
        assert_eq!(formatted, "Just plain text summary");
    }

    // ── collect_key_files ────────────────────────────────────────────

    #[test]
    fn collect_key_files_various_extensions() {
        let messages = vec![make_user(
            "Update src/main.rs and crates/lib.ts plus config/config.json and docs/README.md",
        )];
        let files = collect_key_files(&messages);
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"config/config.json".to_string()));
        assert!(files.contains(&"docs/README.md".to_string()));
    }

    // ── helper unit tests ────────────────────────────────────────────

    #[test]
    fn truncate_summary_short() {
        assert_eq!(truncate_summary("hello", 10), "hello");
    }

    #[test]
    fn truncate_summary_long() {
        let long = "x".repeat(200);
        let truncated = truncate_summary(&long, 160);
        assert!(truncated.ends_with('…'));
        assert!(truncated.chars().count() <= 161);
    }

    #[test]
    fn extract_tag_block_found() {
        let text = "before <foo>content</foo> after";
        assert_eq!(extract_tag_block(text, "foo"), Some("content".to_string()));
    }

    #[test]
    fn extract_tag_block_missing() {
        assert_eq!(extract_tag_block("no tags here", "foo"), None);
    }

    #[test]
    fn strip_tag_block_removes() {
        let text = "before <analysis>junk</analysis> after";
        assert_eq!(strip_tag_block(text, "analysis"), "before  after");
    }

    #[test]
    fn collapse_blank_lines_deduplicates() {
        let text = "a\n\n\nb\n\nc";
        assert_eq!(collapse_blank_lines(text), "a\n\nb\n\nc\n");
    }
}
