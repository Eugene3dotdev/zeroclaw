//! Gemini-family model_provider capabilities and runtime-policy tests.
//!
//! Validates that the direct Gemini API provider is blocked by runtime policy
//! and that the allowed subprocess family uses prompt-guided tools.

use zeroclaw::providers::create_model_provider_with_url;
use zeroclaw::providers::traits::ModelProvider;

fn gemini_cli_model_provider() -> Box<dyn ModelProvider> {
    create_model_provider_with_url("gemini_cli", None, None)
        .expect("Gemini CLI model_provider should resolve without direct API credentials")
}

#[test]
fn direct_gemini_provider_is_blocked_by_policy() {
    let result = create_model_provider_with_url("gemini", Some("test-key"), None);
    assert!(
        result.is_err(),
        "direct Gemini API provider must be blocked"
    );
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("direct provider API runtime"),
        "error should mention direct provider runtime policy: {err_msg}"
    );
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
        "Gemini CLI should use prompt-guided tool calling, not native"
    );
}

#[test]
fn gemini_cli_reports_no_vision_support() {
    let model_provider = gemini_cli_model_provider();
    let caps = model_provider.capabilities();
    assert!(
        !caps.vision,
        "Gemini CLI subprocess adapter should not claim multimodal API support"
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
    let tools = vec![ToolSpec {
        name: "memory_store".to_string(),
        description: "Store a value in memory".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"},
                "value": {"type": "string"}
            },
            "required": ["key", "value"]
        }),
    }];

    let payload = model_provider.convert_tools(&tools);
    assert!(
        matches!(payload, ToolsPayload::PromptGuided { .. }),
        "Gemini CLI should return PromptGuided payload since native_tool_calling is false"
    );
}
