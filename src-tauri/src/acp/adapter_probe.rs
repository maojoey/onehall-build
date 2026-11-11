//! Handshake probe for an installed ACP adapter.
//!
//! An upgrade is only switched in after the new adapter can start, answer
//! `initialize` and open a session. No prompt is sent. An adapter that is up
//! but wants a login answers `session/new` with an auth error; that counts as
//! working, because the install is fine and only credentials are missing.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::Instant;

/// How long a probe may take end to end.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
const STDERR_TAIL: usize = 600;
/// ACP error code for "authentication required".
const AUTH_REQUIRED: i64 = -32000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeReport {
    pub ok: bool,
    /// The adapter works but has no credentials yet.
    pub needs_login: bool,
    pub detail: String,
}

impl ProbeReport {
    fn pass(needs_login: bool, detail: &str) -> Self {
        Self { ok: true, needs_login, detail: detail.into() }
    }
    fn fail(detail: impl Into<String>) -> Self {
        Self { ok: false, needs_login: false, detail: detail.into() }
    }
}

enum Reply {
    Result,
    Error { code: i64, message: String },
    Closed,
    TimedOut,
}

/// Whether a `session/new` error means "the adapter works, log in first".
fn is_auth_error(code: i64, message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    code == AUTH_REQUIRED
        || ["auth", "log in", "login", "sign in", "api key", "credential"]
            .iter()
            .any(|needle| message.contains(needle))
}

/// The script an adapter package starts from: its `bin` entry.
pub fn adapter_entry(package_dir: &Path) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(package_dir.join("package.json"))
        .map_err(|e| format!("cannot read the adapter manifest: {e}"))?;
    let manifest: Value =
        serde_json::from_str(&text).map_err(|e| format!("invalid adapter manifest: {e}"))?;
    let bin = match manifest.get("bin") {
        Some(Value::String(path)) => Some(path.clone()),
        Some(Value::Object(map)) => map.values().find_map(|v| v.as_str().map(String::from)),
        _ => None,
    }
    .ok_or("the adapter manifest has no bin entry")?;
    // A manifest must not point outside its own package.
    if Path::new(&bin).is_absolute() || bin.split(['/', '\\']).any(|part| part == "..") {
        return Err("the adapter bin entry leaves its package".into());
    }
    let entry = package_dir.join(bin);
    entry
        .is_file()
        .then_some(entry)
        .ok_or_else(|| "the adapter bin entry is missing".into())
}

/// Command that starts the adapter at `package_dir` with node.
pub fn adapter_command(node: &Path, package_dir: &Path) -> Result<Command, String> {
    let entry = adapter_entry(package_dir)?;
    let mut command = crate::process::tokio_command(node);
    command.arg(entry);
    Ok(command)
}

async fn request<R: tokio::io::AsyncBufRead + Unpin>(
    stdin: &mut tokio::process::ChildStdin,
    lines: &mut tokio::io::Lines<R>,
    id: u64,
    method: &str,
    params: Value,
    deadline: Instant,
) -> Reply {
    let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let mut line = body.to_string();
    line.push('\n');
    if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
        return Reply::Closed;
    }
    loop {
        let next = tokio::time::timeout_at(deadline, lines.next_line()).await;
        let line = match next {
            Err(_) => return Reply::TimedOut,
            Ok(Ok(Some(line))) => line,
            Ok(_) => return Reply::Closed,
        };
        // Notifications, logs and other ids are not the answer being waited for.
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Reply::Error {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            };
        }
        return Reply::Result;
    }
}

fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_TAIL);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

/// Start `command`, run `initialize` and `session/new` over its stdio, and
/// kill it. `cwd` is the working directory offered to `session/new`.
pub async fn probe_command(mut command: Command, cwd: &Path, timeout: Duration) -> ProbeReport {
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return ProbeReport::fail(format!("cannot start the adapter: {e}")),
    };
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return ProbeReport::fail("the adapter has no stdio");
    };
    let errors = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = errors.clone();
    let reader = tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        while let Ok(n) = stderr.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            let mut held = sink.lock().unwrap();
            held.extend_from_slice(&buffer[..n]);
            let excess = held.len().saturating_sub(STDERR_TAIL * 2);
            held.drain(..excess);
        }
    });
    let mut lines = BufReader::new(stdout).lines();
    let deadline = Instant::now() + timeout;

    let report = match request(
        &mut stdin,
        &mut lines,
        1,
        "initialize",
        json!({ "protocolVersion": 1, "clientCapabilities": {} }),
        deadline,
    )
    .await
    {
        Reply::Result => match request(
            &mut stdin,
            &mut lines,
            2,
            "session/new",
            json!({ "cwd": cwd.to_string_lossy(), "mcpServers": [] }),
            deadline,
        )
        .await
        {
            Reply::Result => ProbeReport::pass(false, "initialize and session/new succeeded"),
            Reply::Error { code, message } if is_auth_error(code, &message) => {
                ProbeReport::pass(true, "the adapter answered and is waiting for a login")
            }
            Reply::Error { message, .. } => ProbeReport::fail(format!("session/new failed: {message}")),
            Reply::Closed => ProbeReport::fail("the adapter exited during session/new"),
            Reply::TimedOut => ProbeReport::fail("session/new timed out"),
        },
        Reply::Error { message, .. } => ProbeReport::fail(format!("initialize failed: {message}")),
        Reply::Closed => ProbeReport::fail("the adapter exited before answering initialize"),
        Reply::TimedOut => ProbeReport::fail("initialize timed out"),
    };
    let _ = child.kill().await;
    reader.abort();
    if report.ok {
        return report;
    }
    let held = errors.lock().unwrap();
    match tail(&held) {
        stderr if stderr.is_empty() => report,
        stderr => ProbeReport::fail(format!("{} ({stderr})", report.detail)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn script(body: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(body);
        command
    }

    const INIT_OK: &str = r#"read a; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}';"#;

    async fn run(body: &str, timeout_ms: u64) -> ProbeReport {
        probe_command(
            script(body),
            &std::env::temp_dir(),
            Duration::from_millis(timeout_ms),
        )
        .await
    }

    #[tokio::test]
    async fn a_working_adapter_passes() {
        let body = format!(
            r#"{INIT_OK} read b; echo '{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"s"}}}}'; sleep 5"#
        );
        let report = run(&body, 5_000).await;
        assert!(report.ok && !report.needs_login, "{report:?}");
    }

    #[tokio::test]
    async fn noise_before_the_answer_is_ignored() {
        let body = r#"read a; echo 'starting up'; echo '{"jsonrpc":"2.0","method":"note","params":{}}'; echo '{"jsonrpc":"2.0","id":99,"result":{}}'; echo '{"jsonrpc":"2.0","id":1,"result":{}}'; read b; echo '{"jsonrpc":"2.0","id":2,"result":{}}'; sleep 5"#;
        assert!(run(body, 5_000).await.ok);
    }

    #[tokio::test]
    async fn a_login_request_still_counts_as_working() {
        let body = format!(
            r#"{INIT_OK} read b; echo '{{"jsonrpc":"2.0","id":2,"error":{{"code":-32000,"message":"Authentication required"}}}}'; sleep 5"#
        );
        let report = run(&body, 5_000).await;
        assert!(report.ok && report.needs_login, "{report:?}");
    }

    #[tokio::test]
    async fn another_session_error_fails() {
        let body = format!(
            r#"{INIT_OK} read b; echo '{{"jsonrpc":"2.0","id":2,"error":{{"code":-32603,"message":"internal boom"}}}}'; sleep 5"#
        );
        let report = run(&body, 5_000).await;
        assert!(!report.ok && report.detail.contains("internal boom"), "{report:?}");
    }

    #[tokio::test]
    async fn a_crashing_adapter_fails_with_its_stderr() {
        let report = run("echo 'module not found' >&2; exit 3", 5_000).await;
        assert!(!report.ok, "{report:?}");
        assert!(report.detail.contains("module not found"), "{report:?}");
    }

    #[tokio::test]
    async fn a_silent_adapter_times_out() {
        let report = run("sleep 5", 300).await;
        assert!(!report.ok && report.detail.contains("timed out"), "{report:?}");
    }

    #[test]
    fn auth_errors_are_recognised_by_code_or_wording() {
        assert!(is_auth_error(-32000, ""));
        assert!(is_auth_error(-32603, "Please log in first"));
        assert!(is_auth_error(1, "missing API key"));
        assert!(!is_auth_error(-32603, "internal error"));
    }

    #[test]
    fn the_entry_must_stay_inside_the_package() {
        let dir = std::env::temp_dir().join(format!("probe-entry-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist/index.js"), "").unwrap();
        let manifest = |bin: &str| {
            std::fs::write(dir.join("package.json"), format!(r#"{{"bin":{bin}}}"#)).unwrap();
        };
        manifest(r#""dist/index.js""#);
        assert_eq!(adapter_entry(&dir).unwrap(), dir.join("dist/index.js"));
        manifest(r#"{"codex-acp":"dist/index.js"}"#);
        assert!(adapter_entry(&dir).is_ok());
        manifest(r#""../outside.js""#);
        assert!(adapter_entry(&dir).is_err());
        manifest(r#""dist/missing.js""#);
        assert!(adapter_entry(&dir).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
