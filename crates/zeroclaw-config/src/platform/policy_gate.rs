//! Fail-closed external policy gate for agent shell commands (fork-only).
//!
//! When `[runtime] policy_gate` names an argv, every shell command a runtime
//! adapter builds is first offered to that program. The gate receives one
//! JSON document on stdin:
//!
//! ```json
//! {"tool_name":"bash","tool_input":{"command":"<full command string>"}}
//! ```
//!
//! Exit status 0 allows the command. Every other outcome refuses it: a
//! non-zero exit, death by signal, a spawn failure (including a missing
//! binary), or running past [`POLICY_GATE_TIMEOUT`]. A refusal is returned as
//! an error from the `build_shell_command*` call, so the caller never gets a
//! process to spawn and there is no path that runs the command anyway.
//!
//! The gate is applied by wrapping the runtime adapter in
//! [`PolicyGatedRuntime`] inside [`super::create_runtime_with_path`], so the
//! shell tool, cron shell jobs, skill shell tools, the coding-CLI executor,
//! and delegates (which share the parent's runtime) all pass through it.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zeroclaw_api::runtime_traits::{RuntimeAdapter, ShellDialect, ShellProfile};

/// Upper bound on one gate invocation. The gate is killed and the command
/// refused when it has not exited by then.
pub const POLICY_GATE_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on the gate stderr carried back to the model as the refusal reason.
const MAX_REASON_BYTES: usize = 8 * 1024;

/// A configured external policy gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyGate {
    argv: Vec<String>,
    timeout: Duration,
}

/// Why the gate refused a command. `Display` is the text the model sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyGateRefusal {
    reason: String,
}

impl std::fmt::Display for PolicyGateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "policy gate refused the command: {}", self.reason)
    }
}

impl std::error::Error for PolicyGateRefusal {}

impl PolicyGateRefusal {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// The refusal reason without the fixed prefix.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl PolicyGate {
    /// Build a gate from the configured argv. An empty argv means no gate.
    pub fn from_argv(argv: &[String]) -> Option<Self> {
        if argv.is_empty() {
            return None;
        }
        Some(Self {
            argv: argv.to_vec(),
            timeout: POLICY_GATE_TIMEOUT,
        })
    }

    /// Override the invocation timeout (tests use a short one).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The JSON document written to the gate's stdin for `command`.
    pub fn payload(command: &str) -> String {
        serde_json::json!({
            "tool_name": "bash",
            "tool_input": { "command": command },
        })
        .to_string()
    }

    /// Ask the gate about `command`, blocking the current thread.
    ///
    /// Inside a multi-threaded Tokio runtime the wait is moved off the async
    /// worker with `block_in_place`.
    pub fn check(&self, command: &str) -> Result<(), PolicyGateRefusal> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.check_blocking(command))
            }
            _ => self.check_blocking(command),
        }
    }

    fn check_blocking(&self, command: &str) -> Result<(), PolicyGateRefusal> {
        let program = self.argv[0].as_str();
        if program.trim().is_empty() {
            return Err(PolicyGateRefusal::new("policy_gate program is empty"));
        }
        let mut child = Command::new(program)
            .args(&self.argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                PolicyGateRefusal::new(format!("policy gate {program:?} could not start: {error}"))
            })?;

        // Feed stdin and drain stderr on their own threads so a gate that
        // ignores stdin or floods stderr cannot wedge the wait below.
        let payload = Self::payload(command);
        let stdin = child.stdin.take();
        let writer = std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                // A gate that exits without reading stdin closes the pipe;
                // its exit status decides, not this write.
                let _ = stdin.write_all(payload.as_bytes());
            }
        });
        let stderr = child.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(stderr) = stderr {
                let _ = stderr
                    .take(MAX_REASON_BYTES as u64 + 1)
                    .read_to_end(&mut buf);
            }
            buf
        });

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() >= deadline => break None,
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PolicyGateRefusal::new(format!(
                        "policy gate {program:?} could not be awaited: {error}"
                    )));
                }
            }
        };
        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            // The writer and reader threads are not joined: a grandchild
            // holding the pipes open must not extend the bounded wait.
            return Err(PolicyGateRefusal::new(format!(
                "policy gate {program:?} timed out after {}s",
                self.timeout.as_secs_f32()
            )));
        };
        let _ = writer.join();
        let stderr = reader.join().unwrap_or_default();

        if status.success() {
            return Ok(());
        }
        let mut reason = String::from_utf8_lossy(&stderr[..stderr.len().min(MAX_REASON_BYTES)])
            .trim()
            .to_string();
        if reason.is_empty() {
            reason = format!("policy gate {program:?} exited with {status}");
        }
        Err(PolicyGateRefusal::new(reason))
    }
}

/// Wrap `runtime` with the configured gate. An empty argv returns it as is.
pub fn wrap_runtime(runtime: Box<dyn RuntimeAdapter>, argv: &[String]) -> Box<dyn RuntimeAdapter> {
    match PolicyGate::from_argv(argv) {
        Some(gate) => Box::new(PolicyGatedRuntime {
            inner: runtime,
            gate,
        }),
        None => runtime,
    }
}

/// A runtime adapter that consults a [`PolicyGate`] before building any shell
/// command. Everything else is delegated unchanged.
pub struct PolicyGatedRuntime {
    inner: Box<dyn RuntimeAdapter>,
    gate: PolicyGate,
}

impl PolicyGatedRuntime {
    pub fn new(inner: Box<dyn RuntimeAdapter>, gate: PolicyGate) -> Self {
        Self { inner, gate }
    }
}

impl RuntimeAdapter for PolicyGatedRuntime {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn has_shell_access(&self) -> bool {
        self.inner.has_shell_access()
    }

    fn has_filesystem_access(&self) -> bool {
        self.inner.has_filesystem_access()
    }

    fn storage_path(&self) -> PathBuf {
        self.inner.storage_path()
    }

    fn supports_long_running(&self) -> bool {
        self.inner.supports_long_running()
    }

    fn memory_budget(&self) -> u64 {
        self.inner.memory_budget()
    }

    fn shell_dialect(&self) -> ShellDialect {
        self.inner.shell_dialect()
    }

    fn shell_profile(&self) -> Option<ShellProfile> {
        self.inner.shell_profile()
    }

    fn shell_program(&self) -> Option<&OsStr> {
        self.inner.shell_program()
    }

    fn build_shell_command(
        &self,
        command: &str,
        workspace_dir: &Path,
    ) -> anyhow::Result<tokio::process::Command> {
        self.gate.check(command)?;
        self.inner.build_shell_command(command, workspace_dir)
    }

    fn build_shell_command_with_effective_path(
        &self,
        command: &str,
        workspace_dir: &Path,
        effective_path: Option<&OsStr>,
    ) -> anyhow::Result<tokio::process::Command> {
        self.gate.check(command)?;
        self.inner
            .build_shell_command_with_effective_path(command, workspace_dir, effective_path)
    }

    fn build_shell_command_with_env_keys(
        &self,
        command: &str,
        workspace_dir: &Path,
        env_keys: &[&OsStr],
    ) -> anyhow::Result<tokio::process::Command> {
        self.gate.check(command)?;
        self.inner
            .build_shell_command_with_env_keys(command, workspace_dir, env_keys)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable `/bin/sh` fixture script and return its path.
    fn fixture(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn gate(argv: &[&str]) -> PolicyGate {
        PolicyGate::from_argv(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .expect("non-empty argv builds a gate")
    }

    fn gated_native(argv: &[&str]) -> Box<dyn RuntimeAdapter> {
        wrap_runtime(
            Box::new(super::super::NativeRuntime::with_shell("/bin/sh".into())),
            &argv.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn policy_gate_payload_matches_hook_contract() {
        let payload: serde_json::Value =
            serde_json::from_str(&PolicyGate::payload("echo \"hi\" | wc -c")).unwrap();
        assert_eq!(
            payload,
            serde_json::json!({"tool_name": "bash", "tool_input": {"command": "echo \"hi\" | wc -c"}})
        );
    }

    #[test]
    fn policy_gate_empty_argv_is_absent() {
        assert!(PolicyGate::from_argv(&[]).is_none());
    }

    #[test]
    fn policy_gate_allows_on_exit_zero_and_receives_payload() {
        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("stdin.json");
        let script = fixture(
            dir.path(),
            "allow",
            &format!("cat > '{}'\nexit 0", seen.display()),
        );
        gate(&[&script, "agent-policy", "zeroclaw", "PreToolUse"])
            .check("ls -la")
            .expect("exit 0 allows");
        let got: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&seen).unwrap()).unwrap();
        assert_eq!(got["tool_name"], "bash");
        assert_eq!(got["tool_input"]["command"], "ls -la");
    }

    #[test]
    fn policy_gate_passes_configured_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let script = fixture(
            dir.path(),
            "args",
            "cat >/dev/null\n[ \"$1 $2 $3\" = \"agent-policy zeroclaw PreToolUse\" ] || { echo \"argv: $*\" >&2; exit 3; }",
        );
        gate(&[&script, "agent-policy", "zeroclaw", "PreToolUse"])
            .check("true")
            .expect("argv reaches the gate");
    }

    #[test]
    fn policy_gate_blocks_on_exit_two_with_stderr_reason() {
        let dir = tempfile::tempdir().unwrap();
        let script = fixture(
            dir.path(),
            "block",
            "cat >/dev/null\necho 'blocked: destructive-git (git reset)' >&2\nexit 2",
        );
        let err = gate(&[&script]).check("git reset --hard").unwrap_err();
        assert_eq!(err.reason(), "blocked: destructive-git (git reset)");
        assert!(err.to_string().contains("blocked: destructive-git"));
    }

    #[test]
    fn policy_gate_blocks_on_nonzero_exit_without_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let script = fixture(dir.path(), "silent", "exit 1");
        let err = gate(&[&script]).check("true").unwrap_err();
        assert!(err.reason().contains("exited with"), "{err}");
    }

    #[test]
    fn policy_gate_blocks_when_binary_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-such-gate");
        let err = gate(&[missing.to_str().unwrap(), "agent-policy"])
            .check("true")
            .unwrap_err();
        assert!(err.reason().contains("could not start"), "{err}");
    }

    #[test]
    fn policy_gate_blocks_on_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let script = fixture(dir.path(), "hang", "exec sleep 30");
        let started = Instant::now();
        let err = gate(&[&script])
            .with_timeout(Duration::from_millis(300))
            .check("true")
            .unwrap_err();
        assert!(err.reason().contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait must be bounded by the gate timeout"
        );
    }

    #[test]
    fn policy_gate_default_timeout_is_bounded() {
        assert_eq!(gate(&["/bin/true"]).timeout, POLICY_GATE_TIMEOUT);
        assert_eq!(POLICY_GATE_TIMEOUT, Duration::from_secs(10));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn policy_gated_runtime_refusal_never_runs_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let gate_script = fixture(dir.path(), "block", "cat >/dev/null\necho 'no' >&2\nexit 2");
        let runtime = gated_native(&[&gate_script]);
        let command = format!("touch '{}'", marker.display());
        for result in [
            runtime.build_shell_command(&command, dir.path()),
            runtime.build_shell_command_with_effective_path(&command, dir.path(), None),
            runtime.build_shell_command_with_env_keys(&command, dir.path(), &[]),
        ] {
            let err = result.expect_err("a refused command yields no process");
            assert!(format!("{err:#}").contains("policy gate refused the command: no"));
        }
        assert!(!marker.exists(), "a refused command must never run");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn policy_gated_runtime_allows_and_runs_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let gate_script = fixture(dir.path(), "allow", "cat >/dev/null\nexit 0");
        let runtime = gated_native(&[&gate_script]);
        let status = runtime
            .build_shell_command(&format!("touch '{}'", marker.display()), dir.path())
            .expect("an allowed command builds")
            .status()
            .await
            .unwrap();
        assert!(status.success());
        assert!(marker.exists());
        assert_eq!(runtime.name(), "native");
    }

    #[test]
    fn policy_gated_runtime_missing_gate_refuses_on_current_thread() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = gated_native(&[dir.path().join("absent").to_str().unwrap()]);
        let err = runtime
            .build_shell_command("true", dir.path())
            .expect_err("a missing gate refuses");
        assert!(format!("{err:#}").contains("could not start"));
    }

    #[test]
    fn create_runtime_applies_configured_policy_gate() {
        let dir = tempfile::tempdir().unwrap();
        let gate_script = fixture(
            dir.path(),
            "block",
            "cat >/dev/null\necho 'denied by fixture' >&2\nexit 2",
        );
        let config = crate::schema::RuntimeConfig {
            shell: Some("/bin/sh".into()),
            policy_gate: vec![gate_script],
            ..Default::default()
        };
        let runtime = super::super::create_runtime(&config).unwrap();
        let err = runtime
            .build_shell_command("true", dir.path())
            .expect_err("create_runtime must apply the configured gate");
        assert!(format!("{err:#}").contains("denied by fixture"));

        let ungated = super::super::create_runtime(&crate::schema::RuntimeConfig {
            shell: Some("/bin/sh".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(ungated.build_shell_command("true", dir.path()).is_ok());
    }
}
