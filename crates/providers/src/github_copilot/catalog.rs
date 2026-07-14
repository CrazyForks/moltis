/// Current GitHub Copilot model IDs. Live `/models` discovery is preferred;
/// this catalog is used when discovery is unavailable.
pub(super) const COPILOT_MODELS: &[(&str, &str)] = &[
    ("gpt-4o", "GPT-4o (Copilot)"),
    ("gpt-4.1", "GPT-4.1 (Copilot)"),
    ("gpt-4.1-mini", "GPT-4.1 Mini (Copilot)"),
    ("gpt-4.1-nano", "GPT-4.1 Nano (Copilot)"),
    ("gpt-5-mini", "GPT-5 mini (Copilot)"),
    ("gpt-5.3-codex", "GPT-5.3-Codex (Copilot)"),
    ("gpt-5.4", "GPT-5.4 (Copilot)"),
    ("gpt-5.4-mini", "GPT-5.4 mini (Copilot)"),
    ("gpt-5.4-nano", "GPT-5.4 nano (Copilot)"),
    ("gpt-5.4-pro", "GPT-5.4 Pro (Copilot)"),
    ("gpt-5.5", "GPT-5.5 (Copilot)"),
    ("gpt-5.6-luna", "GPT-5.6 Luna (Copilot)"),
    ("gpt-5.6-sol", "GPT-5.6 Sol (Copilot)"),
    ("gpt-5.6-terra", "GPT-5.6 Terra (Copilot)"),
    ("gpt-5.2-pro", "GPT-5.2 Pro (Copilot)"),
    ("o1", "o1 (Copilot)"),
    ("o1-mini", "o1-mini (Copilot)"),
    ("o3-mini", "o3-mini (Copilot)"),
    ("claude-fable-5", "Claude Fable 5 (Copilot)"),
    ("claude-haiku-4.5", "Claude Haiku 4.5 (Copilot)"),
    ("claude-opus-4.5", "Claude Opus 4.5 (Copilot)"),
    ("claude-opus-4.6", "Claude Opus 4.6 (Copilot)"),
    ("claude-opus-4.7", "Claude Opus 4.7 (Copilot)"),
    ("claude-opus-4.8", "Claude Opus 4.8 (Copilot)"),
    ("claude-sonnet-4", "Claude Sonnet 4 (Copilot)"),
    ("claude-sonnet-4.5", "Claude Sonnet 4.5 (Copilot)"),
    ("claude-sonnet-4.6", "Claude Sonnet 4.6 (Copilot)"),
    ("claude-sonnet-5", "Claude Sonnet 5 (Copilot)"),
    ("gemini-2.0-flash", "Gemini 2.0 Flash (Copilot)"),
    ("gemini-2.5-pro", "Gemini 2.5 Pro (Copilot)"),
    ("gemini-3-flash-preview", "Gemini 3 Flash (Copilot)"),
    ("gemini-3.1-pro-preview", "Gemini 3.1 Pro (Copilot)"),
    ("gemini-3.5-flash", "Gemini 3.5 Flash (Copilot)"),
    ("mai-code-1-flash-picker", "MAI-Code-1-Flash (Copilot)"),
    ("raptor-mini", "Raptor mini (Copilot)"),
    ("kimi-k2.7-code", "Kimi K2.7 Code (Copilot)"),
];

pub fn default_model_catalog() -> Vec<super::super::DiscoveredModel> {
    super::super::catalog_to_discovered(COPILOT_MODELS, 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_current_copilot_model_ids() {
        for expected in [
            "gpt-5.6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "claude-opus-4.8",
            "claude-sonnet-5",
            "gemini-3-flash-preview",
            "gemini-3.1-pro-preview",
            "mai-code-1-flash-picker",
            "kimi-k2.7-code",
        ] {
            assert!(
                COPILOT_MODELS.iter().any(|(id, _)| *id == expected),
                "missing current Copilot model ID: {expected}"
            );
        }
    }
}
