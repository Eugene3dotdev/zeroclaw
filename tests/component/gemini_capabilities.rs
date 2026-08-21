//! Gemini-family approved-runtime capabilities and contract tests.
//!
//! Validates that the approved Antigravity-backed Gemini CLI model_provider
//! declares its capabilities through the public ModelProvider trait, ensuring
//! the agent loop selects the right tool-calling strategy (prompt-guided, not
//! native).

use zeroclaw::providers::create_model_provider_with_url;
use zeroclaw::providers::traits::ModelProvider;

fn gemini_cli_model_provider() -> Box<dyn ModelProvider> {
    create_model_provider_with_url("gemini-cli", None, None)
        .expect("Gemini CLI model_provider should resolve without direct API credentials")
}

// ─────────────────────────────────────────────────────────────────────────────
// Capabilities declaration
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gemini_cli_reports_no_native_tool_calling() {
    let model_provider = gemini_cli_model_provider();
    let caps = model_provider.capabilities();
    assert!(
        !caps.native_tool_calling,
        "Gemini-family CLI runtime should use prompt-guided tool calling, not native"
    );
}

#[test]
fn gemini_cli_does_not_claim_native_vision_support() {
    let model_provider = gemini_cli_model_provider();
    let caps = model_provider.capabilities();
    assert!(
        !caps.vision,
        "Antigravity subprocess adapter should not claim native vision wire support"
    );
}

#[test]
fn gemini_cli_supports_native_tools_returns_false() {
    let model_provider = gemini_cli_model_provider();
    assert!(
        !model_provider.supports_native_tools(),
        "supports_native_tools() must be false to trigger prompt-guided fallback in chat()"
    );
}

#[test]
fn gemini_cli_supports_vision_returns_false() {
    let model_provider = gemini_cli_model_provider();
    assert!(!model_provider.supports_vision());
}

// ─────────────────────────────────────────────────────────────────────────────
// Tool conversion contract
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gemini_cli_convert_tools_returns_prompt_guided() {
    use zeroclaw::providers::traits::ToolsPayload;
    use zeroclaw::tools::ToolSpec;

    let model_provider = gemini_cli_model_provider();
    let tools = vec![ToolSpec::new(
        "memory_store".to_string(),
        "Store a value in memory".to_string(),
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"},
                "value": {"type": "string"}
            },
            "required": ["key", "value"]
        }),
    )];

    let payload = model_provider.convert_tools(&tools);
    assert!(
        matches!(payload, ToolsPayload::PromptGuided { .. }),
        "Gemini-family CLI runtime should return PromptGuided payload since native_tool_calling is false"
    );
}
