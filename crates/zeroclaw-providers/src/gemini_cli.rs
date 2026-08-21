//! Antigravity CLI subprocess model_provider for Gemini-family runtimes.
//!
//! Integrates with Antigravity, spawning the `agy` binary as a subprocess for
//! each inference request. This keeps Gemini-family access on the approved
//! Antigravity runtime path instead of direct Google AI Studio / Gemini API /
//! Vertex calls.
//!
//! # Usage
//!
//! The `agy` binary must be available in `PATH`, or its location can be
//! set via the typed alias's `binary_path` field.
//!
//! Antigravity CLI is invoked as:
//! ```text
//! agy -p "<prompt>"
//! ```
//! using Antigravity's headless print mode.
//!
//! # Limitations
//!
//! - **Conversation history**: Only the system prompt (if present) and the last
//!   user message are forwarded. Full multi-turn history is not preserved because
//!   the CLI accepts a single prompt per invocation.
//! - **System prompt**: The system prompt is prepended to the user message with a
//!   blank-line separator, as the CLI does not provide a dedicated system-prompt flag.
//! - **Temperature**: The CLI does not expose a temperature parameter.
//!   Only default values are accepted; custom values return an explicit error.
//!
//! # Authentication
//!
//! Authentication is handled by Antigravity itself. Direct Google AI Studio,
//! Gemini API, and Vertex credentials are not accepted by this model_provider.
use crate::traits::{ChatRequest, ChatResponse, ModelProvider, TokenUsage};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

/// Default Antigravity binary name (resolved via `PATH`).
const DEFAULT_ANTIGRAVITY_CLI_BINARY: &str = "agy";

/// Environment variables safe to pass through to the `agy` subprocess.
/// Antigravity authenticates via its own approved session, so direct Google
/// AI Studio / Gemini API / Vertex credentials are never inherited.
const SAFE_ENV_VARS: &[&str] = &[
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SystemRoot",
    "PATH",
    "HOME",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "USER",
    "SHELL",
    "TMPDIR",
];

/// Model name used to signal "use the model_provider's own default model".
const DEFAULT_MODEL_MARKER: &str = "default";
/// Antigravity CLI requests are bounded to avoid hung subprocesses.
const GEMINI_CLI_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Avoid leaking oversized stderr payloads.
const MAX_GEMINI_CLI_STDERR_CHARS: usize = 512;
/// The CLI does not support sampling controls; allow only baseline defaults.
const GEMINI_CLI_SUPPORTED_TEMPERATURES: [f64; 2] = [0.7, 1.0];
const TEMP_EPSILON: f64 = 1e-9;

/// ModelProvider that invokes Antigravity as a subprocess.
///
/// Each inference request spawns a fresh `agy` process. This is the
/// non-interactive approach: the process handles the prompt and exits.
pub struct GeminiCliModelProvider {
    /// `[providers.models.<family>.<alias>]` config-key alias.
    alias: String,
    /// Path to the Antigravity binary.
    binary_path: PathBuf,
}

/// Typed builder for [`GeminiCliModelProvider`].
///
/// Only `alias` is required; the binary path defaults to the module's
/// `DEFAULT_ANTIGRAVITY_CLI_BINARY` (PATH lookup) when unset.
#[must_use]
pub struct GeminiCliBuilder {
    alias: String,
    binary_path: Option<String>,
}

impl GeminiCliBuilder {
    /// Override the Antigravity CLI binary path. Whitespace-only inputs
    /// fall back to the default (PATH lookup).
    pub fn binary_path(mut self, path: Option<&str>) -> Self {
        self.binary_path = path
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string);
        self
    }

    pub fn build(self) -> GeminiCliModelProvider {
        let binary_path = self
            .binary_path
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ANTIGRAVITY_CLI_BINARY));
        GeminiCliModelProvider {
            alias: self.alias,
            binary_path,
        }
    }
}

impl GeminiCliModelProvider {
    /// Entry point. Only `alias` is required; every other field is set
    /// via a labelled chain method on the returned [`GeminiCliBuilder`].
    pub fn builder(alias: &str) -> GeminiCliBuilder {
        GeminiCliBuilder {
            alias: alias.to_string(),
            binary_path: None,
        }
    }

    /// Returns true if the model argument should be forwarded to the CLI.
    fn should_forward_model(model: &str) -> bool {
        let trimmed = model.trim();
        !trimmed.is_empty() && trimmed != DEFAULT_MODEL_MARKER
    }

    fn supports_temperature(temperature: f64) -> bool {
        GEMINI_CLI_SUPPORTED_TEMPERATURES
            .iter()
            .any(|v| (temperature - v).abs() < TEMP_EPSILON)
    }

    fn validate_temperature(temperature: f64) -> anyhow::Result<()> {
        if !temperature.is_finite() {
            anyhow::bail!("Antigravity model_provider received non-finite temperature value");
        }
        if !Self::supports_temperature(temperature) {
            anyhow::bail!(
                "temperature unsupported by Antigravity CLI: {temperature}. \
                 Supported values: 0.7 or 1.0"
            );
        }
        Ok(())
    }

    fn redact_stderr(stderr: &[u8]) -> String {
        let text = String::from_utf8_lossy(stderr);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        if trimmed.chars().count() <= MAX_GEMINI_CLI_STDERR_CHARS {
            return trimmed.to_string();
        }
        let clipped: String = trimmed.chars().take(MAX_GEMINI_CLI_STDERR_CHARS).collect();
        format!("{clipped}...")
    }

    /// Build the argv tail (excluding the binary itself) for a CLI invocation.
    ///
    /// Antigravity's headless print mode takes the prompt as the trailing
    /// positional argument. Model flags are emitted first so a parser that
    /// stops option parsing at the first positional still receives `--model`.
    /// The CLI has no stdin-fed prompt form.
    fn build_cli_args(message: &str, model: &str) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        if Self::should_forward_model(model) {
            args.extend(["--model".to_string(), model.to_string()]);
        }
        args.extend(["-p".to_string(), message.to_string()]);
        args
    }

    /// Invoke Antigravity with the given prompt and optional model.
    /// Returns the trimmed stdout output as the assistant response.
    async fn invoke_cli(&self, message: &str, model: &str) -> anyhow::Result<String> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.args(Self::build_cli_args(message, model));

        // Direct Google AI Studio / Gemini API / Vertex credentials in the
        // parent process must not reach the session-auth runtime. Restore
        // only the safe process variables the CLI needs.
        cmd.env_clear();
        for var in SAFE_ENV_VARS {
            if let Ok(val) = std::env::var(*var) {
                cmd.env(*var, val);
            }
        }

        cmd.kill_on_drop(true);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let child = cmd.spawn().map_err(|err| {
            ::zeroclaw_log::record!(
                ERROR,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                    .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                    .with_attrs(::serde_json::json!({
                        "binary": self.binary_path.display().to_string(),
                        "phase": "spawn",
                        "error": format!("{}", err),
                    })),
                "gemini_cli: failed to spawn binary"
            );
            anyhow::Error::msg(format!(
                "Failed to spawn Antigravity CLI binary at {}: {err}. \
                 Ensure `agy` is installed and in PATH, or set the alias binary_path.",
                self.binary_path.display()
            ))
        })?;

        let output = timeout(GEMINI_CLI_REQUEST_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                ::zeroclaw_log::record!(
                    WARN,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Timeout)
                        .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                        .with_attrs(::serde_json::json!({
                            "binary": self.binary_path.display().to_string(),
                            "timeout": format!("{:?}", GEMINI_CLI_REQUEST_TIMEOUT),
                        })),
                    "gemini_cli: Antigravity request timed out"
                );
                anyhow::Error::msg(format!(
                    "Antigravity CLI request timed out after {:?} (binary: {})",
                    GEMINI_CLI_REQUEST_TIMEOUT,
                    self.binary_path.display()
                ))
            })?
            .map_err(|err| {
                ::zeroclaw_log::record!(
                    ERROR,
                    ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                        .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                        .with_attrs(::serde_json::json!({
                            "phase": "process_wait",
                            "error": format!("{}", err),
                        })),
                    "gemini_cli: Antigravity process wait failed"
                );
                anyhow::Error::msg(format!("Antigravity CLI process failed: {err}"))
            })?;

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            let stderr_excerpt = Self::redact_stderr(&output.stderr);
            let stderr_note = if stderr_excerpt.is_empty() {
                String::new()
            } else {
                format!(" Stderr: {stderr_excerpt}")
            };
            anyhow::bail!(
                "Antigravity CLI exited with non-zero status {code}. \
                 Check that Antigravity is authenticated and the CLI is supported.{stderr_note}"
            );
        }

        let text = String::from_utf8(output.stdout).map_err(|err| {
            ::zeroclaw_log::record!(
                ERROR,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                    .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                    .with_attrs(::serde_json::json!({
                        "phase": "utf8_decode",
                        "error": format!("{}", err),
                    })),
                "gemini_cli: Antigravity produced non-UTF-8 stdout"
            );
            anyhow::Error::msg(format!("Antigravity CLI produced non-UTF-8 output: {err}"))
        })?;

        Ok(text.trim().to_string())
    }
}

#[async_trait]
impl ModelProvider for GeminiCliModelProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: Option<f64>,
    ) -> anyhow::Result<String> {
        if let Some(t) = temperature {
            Self::validate_temperature(t)?;
        }

        let full_message = match system_prompt {
            Some(system) if !system.is_empty() => {
                format!("{system}\n\n{message}")
            }
            _ => message.to_string(),
        };

        self.invoke_cli(&full_message, model).await
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        model: &str,
        temperature: Option<f64>,
    ) -> anyhow::Result<ChatResponse> {
        let text = self
            .chat_with_history(request.messages, model, temperature)
            .await?;

        Ok(ChatResponse {
            text: Some(text),
            tool_calls: Vec::new(),
            usage: Some(TokenUsage::default()),
            reasoning_content: None,
        })
    }
}

impl ::zeroclaw_api::attribution::Attributable for GeminiCliModelProvider {
    fn role(&self) -> ::zeroclaw_api::attribution::Role {
        ::zeroclaw_api::attribution::Role::Provider(
            ::zeroclaw_api::attribution::ProviderKind::Model(
                ::zeroclaw_api::attribution::ModelProviderKind::GeminiCli,
            ),
        )
    }
    fn alias(&self) -> &str {
        &self.alias
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_uses_explicit_binary_path() {
        let p = GeminiCliModelProvider::builder("test")
            .binary_path(Some("/usr/local/bin/agy"))
            .build();
        assert_eq!(p.binary_path, PathBuf::from("/usr/local/bin/agy"));
    }

    #[test]
    fn new_defaults_to_antigravity() {
        let p = GeminiCliModelProvider::builder("test")
            .binary_path(None)
            .build();
        assert_eq!(p.binary_path, PathBuf::from("agy"));
    }

    #[test]
    fn new_ignores_blank_binary_path() {
        let p = GeminiCliModelProvider::builder("test")
            .binary_path(Some("   "))
            .build();
        assert_eq!(p.binary_path, PathBuf::from("agy"));
    }

    #[test]
    fn should_forward_model_standard() {
        assert!(GeminiCliModelProvider::should_forward_model(
            "gemini-2.5-pro"
        ));
        assert!(GeminiCliModelProvider::should_forward_model(
            "gemini-2.5-flash"
        ));
    }

    #[test]
    fn should_not_forward_default_model() {
        assert!(!GeminiCliModelProvider::should_forward_model(
            DEFAULT_MODEL_MARKER
        ));
        assert!(!GeminiCliModelProvider::should_forward_model(""));
        assert!(!GeminiCliModelProvider::should_forward_model("   "));
    }

    #[test]
    fn validate_temperature_allows_defaults() {
        assert!(GeminiCliModelProvider::validate_temperature(0.7).is_ok());
        assert!(GeminiCliModelProvider::validate_temperature(1.0).is_ok());
    }

    #[test]
    fn validate_temperature_rejects_custom_value() {
        let err = GeminiCliModelProvider::validate_temperature(0.2).unwrap_err();
        assert!(
            err.to_string()
                .contains("temperature unsupported by Antigravity CLI")
        );
    }

    #[test]
    fn build_cli_args_uses_antigravity_print_mode_for_default_model() {
        let args = GeminiCliModelProvider::build_cli_args("hello", DEFAULT_MODEL_MARKER);
        assert_eq!(args, vec!["-p".to_string(), "hello".to_string()]);
    }

    #[test]
    fn build_cli_args_places_explicit_model_before_print_prompt() {
        let args = GeminiCliModelProvider::build_cli_args("hello", "gemini-2.5-pro");
        assert_eq!(
            args,
            vec![
                "--model".to_string(),
                "gemini-2.5-pro".to_string(),
                "-p".to_string(),
                "hello".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn invoke_missing_binary_returns_error() {
        let model_provider = GeminiCliModelProvider {
            alias: "test".to_string(),
            binary_path: PathBuf::from("/nonexistent/path/to/agy"),
        };
        let result = model_provider.invoke_cli("hello", "default").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Failed to spawn Antigravity CLI binary"),
            "unexpected error message: {msg}"
        );
    }
}
