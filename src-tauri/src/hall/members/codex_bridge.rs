//! Team-only compatibility bridge for codex-acp / Codex CLI 0.163.0 and newer.
//! The adapter supplies a broad sandbox preset on every turn. Keep the host's
//! frozen native permissions instead, including on auxiliary title threads.
//! No request contents, credentials, or frozen configuration are logged.
use crate::app_error::AppCommandError;
#[cfg(any(feature = "cli-bin", test))]
use serde_json::json;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(feature = "cli-bin")]
use std::process::ExitCode;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
#[cfg(any(feature = "cli-bin", test))]
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;

const ENABLED: &str = "ONEHALL_CODEX_BRIDGE";
const PROGRAM: &str = "ONEHALL_CODEX_BRIDGE_PROGRAM";
const CONFIG: &str = "ONEHALL_CODEX_BRIDGE_CONFIG";
/// Node binary that runs [`PROGRAM`] when it is a `.js` entry (the CLI that
/// ships inside codex-acp); absent for a native executable.
const RUNNER: &str = "ONEHALL_CODEX_BRIDGE_RUNNER";
/// The release the frozen-permission behaviour was verified against. It is what
/// the execution policy records; launching accepts any newer 0.x release too
/// (see [`version_supported`]), because the CLI updates faster than we re-verify.
pub(crate) const SUPPORTED_VERSION: &str = "codex-cli 0.163.0";
const MIN_CODEX_MINOR: u64 = 163;
#[cfg(any(feature = "cli-bin", test))]
const MAX_LINE: usize = 4 * 1024 * 1024;
const MAX_CONFIG: usize = 512 * 1024;
const FAILURE: &str = "OneHall Codex bridge rejected an unsupported or invalid protocol request";

fn invalid(message: &str) -> AppCommandError {
    AppCommandError::invalid_input(message)
}

/// A Codex CLI the bridge may launch: the program, plus the node binary to run
/// it with when it is a `.js` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cli {
    program: PathBuf,
    runner: Option<PathBuf>,
}

/// Executable and leading arguments that start `program`, through `runner`
/// when there is one.
fn launch_parts(runner: Option<&Path>, program: &Path) -> (PathBuf, Vec<OsString>) {
    match runner {
        Some(runner) => (runner.to_path_buf(), vec![program.as_os_str().to_owned()]),
        None => (program.to_path_buf(), Vec::new()),
    }
}

/// CLIs to try, best first. An explicit `CODEX_PATH` is the user's choice and
/// is the only candidate. Otherwise the CLI bundled with codex-acp comes first
/// (it is the one the adapter was built against), then `codex` from PATH.
fn ordered_candidates(
    explicit: Option<&str>,
    bundled: Option<&crate::acp::adapter_cli::BundledCli>,
    node: Option<&Path>,
    path: &std::ffi::OsStr,
    cwd: Option<&str>,
) -> Vec<Cli> {
    let runner_for = |program: &Path| {
        program
            .extension()
            .is_some_and(|ext| ext == "js")
            .then(|| node.map(Path::to_path_buf))
    };
    let make = |program: PathBuf| -> Option<Cli> {
        match runner_for(&program) {
            Some(Some(runner)) => Some(Cli { program, runner: Some(runner) }),
            Some(None) => None, // a .js entry with no node to run it
            None => Some(Cli { program, runner: None }),
        }
    };
    if let Some(name) = explicit {
        return resolve_program(name, path, cwd)
            .and_then(make)
            .into_iter()
            .collect();
    }
    let mut out = Vec::new();
    if let Some(cli) = bundled
        .and_then(|cli| std::fs::canonicalize(&cli.path).ok())
        .and_then(make)
    {
        out.push(cli);
    }
    if let Some(cli) = resolve_program("codex", path, cwd).and_then(make) {
        if !out.contains(&cli) {
            out.push(cli);
        }
    }
    out
}

/// Called only for host-prepared hall Codex launches, after member overrides.
/// Select the Codex CLI (see [`ordered_candidates`]) before substituting our
/// own same-release CLI.
pub(crate) async fn prepare(
    runtime: &mut BTreeMap<String, String>,
    cwd: Option<&str>,
    frozen_config: &str,
) -> Result<(), AppCommandError> {
    let own = crate::cli_run::shim::layout()
        .map(|layout| layout.program().to_path_buf())
        .ok_or_else(|| invalid("Hall Codex launch requires this OneHall release's codeg-cli"))?;
    let own = std::fs::canonicalize(own)
        .map_err(|_| invalid("This OneHall release's codeg-cli is unavailable"))?;
    let explicit = runtime
        .get("CODEX_PATH")
        .cloned()
        .or_else(|| std::env::var("CODEX_PATH").ok())
        .filter(|value| !value.trim().is_empty());
    let path = runtime
        .get("PATH")
        .map(OsString::from)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let bundled = if explicit.is_none() {
        crate::acp::adapter_cli::installed_adapter(crate::acp::adapter_cli::AdapterKind::Codex)
            .await
            .and_then(|adapter| adapter.cli)
    } else {
        None
    };
    let node = which::which_in("node", Some(&path), cwd.unwrap_or(".")).ok();
    let candidates = ordered_candidates(explicit.as_deref(), bundled.as_ref(), node.as_deref(), &path, cwd);
    if candidates.is_empty() {
        return Err(invalid(
            "Hall Codex launch requires an installed Codex CLI 0.163.0 or newer",
        ));
    }
    let mut chosen = None;
    for cli in candidates {
        if cli.program == own {
            return Err(invalid("Hall Codex bridge cannot invoke itself"));
        }
        match verify_version(&cli).await {
            Ok(()) => {
                chosen = Some(cli);
                break;
            }
            Err(error) => tracing::warn!(
                program = %cli.program.display(),
                "skipping a Codex CLI that failed the bridge version check: {error}"
            ),
        }
    }
    let actual = chosen.ok_or_else(|| {
        invalid("Hall Codex bridge needs Codex CLI 0.163.0 or newer; check CODEX_PATH or the installed codex")
    })?;
    // Only host-generated member configuration defines the lock. Runtime
    // settings may contain legacy profiles or dotted overrides and are never
    // used as the source of trusted restrictions.
    let locked = Locked::parse(frozen_config).map_err(invalid)?;
    let mut adapter_config = runtime
        .get(super::launch::CODEX_CONFIG)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| invalid("Hall Codex launch has no native configuration"))?;
    locked.apply_config(&mut adapter_config);
    runtime.insert(
        super::launch::CODEX_CONFIG.into(),
        Value::Object(adapter_config).to_string(),
    );
    runtime.insert(CONFIG.into(), Value::Object(locked.config).to_string());
    runtime.insert(PROGRAM.into(), actual.program.to_string_lossy().into_owned());
    match &actual.runner {
        Some(runner) => runtime.insert(RUNNER.into(), runner.to_string_lossy().into_owned()),
        None => runtime.remove(RUNNER),
    };
    runtime.insert(ENABLED.into(), "1".into());
    runtime.insert("CODEX_PATH".into(), own.to_string_lossy().into_owned());
    Ok(())
}

fn resolve_program(name: &str, path: &std::ffi::OsStr, cwd: Option<&str>) -> Option<PathBuf> {
    let candidate = Path::new(name);
    let candidates = if candidate.is_absolute() {
        vec![candidate.to_path_buf()]
    } else if candidate.components().count() > 1 {
        vec![Path::new(cwd?).join(candidate)]
    } else {
        std::env::split_paths(path)
            .flat_map(|dir| {
                let p = dir.join(name);
                #[cfg(windows)]
                {
                    vec![p.clone(), p.with_extension("exe"), p.with_extension("cmd")]
                }
                #[cfg(not(windows))]
                {
                    vec![p]
                }
            })
            .collect()
    };
    candidates.into_iter().find_map(|p| {
        let p = std::fs::canonicalize(p).ok()?;
        p.is_file().then_some(p)
    })
}

/// `codex-cli 0.A.B` with A at least [`MIN_CODEX_MINOR`]. Newer releases are
/// accepted so an upstream CLI update does not take Hall Codex members down;
/// a CLI that no longer honours the frozen configuration fails at launch.
fn version_supported(output: &str) -> bool {
    let Some(version) = output.trim().strip_prefix("codex-cli ") else {
        return false;
    };
    let mut parts = version.split('.').map(|p| p.parse::<u64>());
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(Ok(0)), Some(Ok(minor)), Some(Ok(_)), None) if minor >= MIN_CODEX_MINOR
    )
}

async fn verify_version(cli: &Cli) -> Result<(), AppCommandError> {
    let (exe, lead) = launch_parts(cli.runner.as_deref(), &cli.program);
    let mut child = Command::new(exe)
        .args(lead)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| invalid("Cannot execute the selected Codex CLI"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let check = async {
        let mut bytes = Vec::new();
        stdout.take(257).read_to_end(&mut bytes).await?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>(
            status.success()
                && bytes.len() <= 256
                && std::str::from_utf8(&bytes).is_ok_and(version_supported),
        )
    };
    if !matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), check).await,
        Ok(Ok(true))
    ) {
        let _ = child.kill().await;
        return Err(invalid(
            "Hall Codex bridge needs Codex CLI 0.163.0 or newer; check CODEX_PATH or the installed codex",
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct Locked {
    config: Map<String, Value>,
}

fn protected(key: &str) -> bool {
    // TOML keyPath accepts quoted components. Refuse ambiguous spellings rather
    // than allowing a quoted permission root to bypass the write guard.
    if key.contains(['"', '\'', '[', ']']) {
        return true;
    }
    matches!(
        key.split('.').next().unwrap_or(key).trim(),
        "default_permissions"
            | "permissions"
            | "sandbox_mode"
            | "sandbox_workspace_write"
            | "profile"
            | "profiles"
            | "shell_environment_policy"
            | "allow_login_shell"
            | "web_search"
            | "features"
            | "hooks"
            | "apps"
            | "plugins"
            | "notify"
            | "developer_instructions"
    )
}

fn frozen_key(key: &str) -> bool {
    matches!(
        key,
        "default_permissions"
            | "permissions"
            | "shell_environment_policy"
            | "allow_login_shell"
            | "web_search"
            | "features"
            | "hooks"
            | "apps"
            | "notify"
            | "developer_instructions"
    )
}

impl Locked {
    fn parse(raw: &str) -> Result<Self, &'static str> {
        if raw.len() > MAX_CONFIG {
            return Err(FAILURE);
        }
        let value: Value = serde_json::from_str(raw).map_err(|_| FAILURE)?;
        let config = value.as_object().ok_or(FAILURE)?;
        if config.get("default_permissions").and_then(Value::as_str) != Some("onehall-member")
            || !config.get("permissions").is_some_and(Value::is_object)
            || !config.get("features").is_some_and(Value::is_object)
            || !config
                .get("shell_environment_policy")
                .is_some_and(Value::is_object)
        {
            return Err(FAILURE);
        }
        Ok(Self {
            config: config
                .iter()
                .filter(|(key, _)| frozen_key(key))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }

    fn apply_config(&self, config: &mut Map<String, Value>) {
        let mut features = config
            .get("features")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        features.extend(
            self.config
                .get("features")
                .and_then(Value::as_object)
                .expect("validated features")
                .clone(),
        );
        config.retain(|key, _| !protected(key));
        config.extend(self.config.clone());
        config.insert("features".into(), Value::Object(features));
    }

    #[cfg(any(feature = "cli-bin", test))]
    fn filter(&self, bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
        if bytes.len() > MAX_LINE {
            return Err(FAILURE);
        }
        let mut message: Value = serde_json::from_slice(bytes).map_err(|_| FAILURE)?;
        let root = message.as_object_mut().ok_or(FAILURE)?;
        let Some(method) = root
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            // Responses to app-server requests (including approvals) are opaque.
            return Ok(bytes.to_vec());
        };
        match method.as_str() {
            "thread/start" | "thread/resume" | "thread/fork" => {
                let params = root
                    .get_mut("params")
                    .and_then(Value::as_object_mut)
                    .ok_or(FAILURE)?;
                remove_overrides(params);
                let config = params.entry("config").or_insert_with(|| json!({}));
                if config.is_null() {
                    *config = json!({});
                }
                let config = config.as_object_mut().ok_or(FAILURE)?;
                self.apply_config(config);
            }
            "turn/start" | "thread/settings/update" => {
                let params = root
                    .get_mut("params")
                    .and_then(Value::as_object_mut)
                    .ok_or(FAILURE)?;
                remove_overrides(params);
                // These methods do not take general configuration. Reject an
                // unexpected config field instead of silently accepting a new
                // version's override surface.
                if params.contains_key("config") {
                    return Err(FAILURE);
                }
            }
            "config/value/write" => {
                let params = root
                    .get("params")
                    .and_then(Value::as_object)
                    .ok_or(FAILURE)?;
                if protected(
                    params
                        .get("keyPath")
                        .and_then(Value::as_str)
                        .ok_or(FAILURE)?,
                ) {
                    return Err(FAILURE);
                }
            }
            "config/batchWrite" => {
                let edits = root
                    .get("params")
                    .and_then(|p| p.get("edits"))
                    .and_then(Value::as_array)
                    .ok_or(FAILURE)?;
                for edit in edits {
                    if protected(edit.get("keyPath").and_then(Value::as_str).ok_or(FAILURE)?) {
                        return Err(FAILURE);
                    }
                }
            }
            // These channels can run outside any thread's frozen permissions.
            "command/exec" | "thread/shellCommand" => return Err(FAILURE),
            _ => return Ok(bytes.to_vec()),
        }
        serde_json::to_vec(&message).map_err(|_| FAILURE)
    }

    #[cfg(any(feature = "cli-bin", test))]
    fn startup_args(&self, original: &[OsString]) -> Result<Vec<OsString>, &'static str> {
        if original.first().and_then(|arg| arg.to_str()) != Some("app-server") {
            return Err(FAILURE);
        }
        // Keep adapter hook configuration arguments, then override every
        // protected setting. Only stdio app-server transport is supported.
        let mut index = 1;
        while index < original.len() {
            match original[index].to_str() {
                Some("-c" | "--config") if index + 1 < original.len() => index += 2,
                Some("--listen")
                    if original.get(index + 1).and_then(|a| a.to_str()) == Some("stdio://") =>
                {
                    index += 2
                }
                _ => return Err(FAILURE),
            }
        }
        let mut args = original.to_vec();
        for (key, value) in &self.config {
            // Manuals remain solely in env and thread configuration. No key
            // value is ever in this configuration in the first place.
            if key == "developer_instructions" {
                continue;
            }
            let value = inline_toml(value)?;
            if value.len() > 96 * 1024 {
                return Err(FAILURE);
            }
            args.extend([
                OsString::from("-c"),
                OsString::from(format!("{key}={value}")),
            ]);
        }
        Ok(args)
    }
}

#[cfg(any(feature = "cli-bin", test))]
fn remove_overrides(params: &mut Map<String, Value>) {
    for key in [
        "sandbox",
        "sandboxPolicy",
        "sandbox_mode",
        "sandbox_workspace_write",
        "permissionProfile",
        "permissionProfileName",
        "permissions",
        "default_permissions",
        "profile",
        "profiles",
    ] {
        params.remove(key);
    }
}

#[cfg(any(feature = "cli-bin", test))]
fn inline_toml(value: &Value) -> Result<String, &'static str> {
    match value {
        Value::String(_) | Value::Bool(_) | Value::Number(_) => {
            serde_json::to_string(value).map_err(|_| FAILURE)
        }
        Value::Array(items) => Ok(format!(
            "[{}]",
            items
                .iter()
                .map(inline_toml)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        Value::Object(items) => Ok(format!(
            "{{{}}}",
            items
                .iter()
                .map(|(key, value)| Ok(format!(
                    "{}={}",
                    serde_json::to_string(key).map_err(|_| FAILURE)?,
                    inline_toml(value)?
                )))
                .collect::<Result<Vec<_>, &'static str>>()?
                .join(",")
        )),
        Value::Null => Err(FAILURE),
    }
}

#[cfg(any(feature = "cli-bin", test))]
async fn forward<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: R,
    mut writer: W,
    locked: &Locked,
) -> Result<(), &'static str> {
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = Vec::new();
        // `read_until` alone grows without a bound on an unterminated line.
        let count = (&mut reader)
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|_| FAILURE)?;
        if count == 0 {
            writer.shutdown().await.map_err(|_| FAILURE)?;
            return Ok(());
        }
        if count > MAX_LINE || line.last() != Some(&b'\n') {
            return Err(FAILURE);
        }
        let bytes = locked.filter(&line)?;
        writer.write_all(&bytes).await.map_err(|_| FAILURE)?;
        if bytes.last() != Some(&b'\n') {
            writer.write_all(b"\n").await.map_err(|_| FAILURE)?;
        }
        writer.flush().await.map_err(|_| FAILURE)?;
    }
}

#[cfg(any(feature = "cli-bin", test))]
async fn cancelled() -> i32 {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut int = signal(SignalKind::interrupt()).ok();
        let mut term = signal(SignalKind::terminate()).ok();
        let recv = |value: Option<tokio::signal::unix::Signal>| async move {
            if let Some(mut value) = value {
                value.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! { _ = recv(int.take()) => 130, _ = recv(term.take()) => 143 }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        130
    }
}

#[cfg(any(feature = "cli-bin", test))]
async fn relay<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut command: Command,
    input: R,
    mut output: W,
    locked: Locked,
) -> Result<i32, &'static str> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| FAILURE)?;
    let stdin = child.stdin.take().ok_or(FAILURE)?;
    let mut stdout = child.stdout.take().ok_or(FAILURE)?;
    let incoming = forward(input, stdin, &locked);
    let outgoing = tokio::io::copy(&mut stdout, &mut output);
    tokio::pin!(incoming, outgoing);
    let result = tokio::select! {
        value = &mut incoming => {
            match value {
                Err(error) => Err(error),
                Ok(()) => {
                    // EOF closes child stdin. Drain final protocol bytes, but
                    // never wait forever for a child that ignores EOF.
                    match tokio::time::timeout(std::time::Duration::from_secs(2), &mut outgoing).await {
                        Ok(Err(_)) => Err(FAILURE),
                        _ => Ok(0),
                    }
                }
            }
        }
        value = &mut outgoing => value.map(|_|0).map_err(|_|FAILURE),
        code = cancelled() => Ok(code),
    };
    // Every path, including protocol rejection, owns child cleanup. Preserve a
    // naturally exited child's status; kill a child that is still running.
    let mut status = child.try_wait().ok().flatten();
    if status.is_none() && matches!(result, Ok(0)) {
        // stdout EOF can precede the OS exit notification by a few moments.
        status = tokio::time::timeout(std::time::Duration::from_millis(250), child.wait())
            .await
            .ok()
            .and_then(Result::ok);
    }
    if let Some(status) = status {
        return result.map(|code| {
            if code == 0 {
                status.code().unwrap_or(1)
            } else {
                code
            }
        });
    }
    let _ = child.kill().await;
    result
}

/// Hidden entry before CLI argument parsing. An ordinary codeg-cli invocation
/// never takes this branch. All output on stdout belongs to the real protocol.
#[cfg(feature = "cli-bin")]
pub(crate) fn entry() -> Option<ExitCode> {
    if std::env::var(ENABLED).ok().as_deref() != Some("1") {
        return None;
    }
    let result = || -> Result<i32, &'static str> {
        let program = std::env::var_os(PROGRAM).ok_or(FAILURE)?;
        let own = std::env::current_exe().map_err(|_| FAILURE)?;
        let actual = std::fs::canonicalize(&program).map_err(|_| FAILURE)?;
        if actual == std::fs::canonicalize(own).map_err(|_| FAILURE)? {
            return Err(FAILURE);
        }
        let runner = std::env::var_os(RUNNER).map(PathBuf::from);
        let (exe, lead) = launch_parts(runner.as_deref(), &actual);
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args == [OsString::from("--version")] {
            return std::process::Command::new(&exe)
                .args(&lead)
                .arg("--version")
                .env_remove(ENABLED)
                .env_remove(CONFIG)
                .env_remove(PROGRAM)
                .env_remove(RUNNER)
                .status()
                .map(|status| status.code().unwrap_or(1))
                .map_err(|_| FAILURE);
        }
        let locked = Locked::parse(&std::env::var(CONFIG).map_err(|_| FAILURE)?)?;
        let args = locked.startup_args(&args)?;
        let mut command = Command::new(&exe);
        command
            .args(lead)
            .args(args)
            .env_remove(ENABLED)
            .env_remove(CONFIG)
            .env_remove(PROGRAM)
            .env_remove(RUNNER)
            .env("CODEX_PATH", &actual);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| FAILURE)?;
        let result = runtime.block_on(relay(
            command,
            tokio::io::stdin(),
            tokio::io::stdout(),
            locked,
        ));
        // Tokio's stdin uses a blocking read. Do not join it when the child
        // exits before the adapter closes stdin; returning exits this process.
        runtime.shutdown_background();
        result
    }();
    Some(match result {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(message) => {
            eprintln!("codeg-cli: {message}");
            ExitCode::FAILURE
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locked() -> Locked {
        Locked::parse(&json!({
            "default_permissions":"onehall-member",
            "permissions":{"onehall-member":{"filesystem":{":root":"deny","/member":"read","/member/.onedesk":"deny"},"network":{"enabled":false}}},
            "features":{"network_proxy":false,"plugins":false,"apps":false,"hooks":false,"multi_agent":false},
            "shell_environment_policy":{"inherit":"all","include_only":["PATH"]},
            "allow_login_shell":false,"web_search":"disabled", "hooks":{},
            "developer_instructions":"private manual fixture"
        }).to_string()).unwrap()
    }
    fn filter(value: Value) -> Value {
        serde_json::from_slice(
            &locked()
                .filter(&serde_json::to_vec(&value).unwrap())
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn every_thread_pins_frozen_permissions_including_auxiliary_title_threads() {
        for method in ["thread/start", "thread/resume", "thread/fork"] {
            let request = json!({"id":17,"method":method,"params":{
                "cwd":"/member","ephemeral":true,"model":"keep-model","sandbox":"danger-full-access",
                "permissionProfile":"unsafe","approvalPolicy":"on-request","approvalsReviewer":"user",
                "config":{"default_permissions":"unsafe","permissions":{"unsafe":{}},"sandbox_mode":"danger-full-access", "sandbox_workspace_write":{"writable_roots":["/"]},"profile":"global", "profiles":{},
                    "features":{"plugins":true,"network_proxy":true,"cwd_relative_turn_diffs":false},
                    "features.plugins":true,"shell_environment_policy":{"inherit":"all"},
                    "model":"keep-config-model","mcp_servers":{"codeg":{"command":"keep-mcp"}}}
            }});
            let filtered = filter(request);
            assert_eq!(filtered["id"], 17);
            let p = &filtered["params"];
            assert_eq!(p["model"], "keep-model");
            assert_eq!(p["approvalPolicy"], "on-request");
            assert_eq!(p["approvalsReviewer"], "user");
            assert!(p.get("sandbox").is_none());
            assert!(p.get("permissionProfile").is_none());
            let config = &p["config"];
            assert_eq!(config["permissions"], locked().config["permissions"]);
            assert_eq!(config["default_permissions"], "onehall-member");
            assert_eq!(config["features"]["plugins"], false);
            assert_eq!(config["features"]["network_proxy"], false);
            assert_eq!(config["features"]["cwd_relative_turn_diffs"], false);
            assert_eq!(config["model"], "keep-config-model");
            assert_eq!(config["mcp_servers"]["codeg"]["command"], "keep-mcp");
            for legacy in [
                "sandbox_mode",
                "sandbox_workspace_write",
                "profile",
                "profiles",
                "features.plugins",
            ] {
                assert!(config.get(legacy).is_none());
            }
        }
        let title = filter(
            json!({"id":"title","method":"thread/start","params":{"cwd":"/member","ephemeral":true}}),
        );
        assert_eq!(
            title["params"]["config"]["permissions"],
            locked().config["permissions"]
        );
    }

    #[test]
    fn turns_drop_adapter_presets_without_touching_approvals_or_model_payload() {
        let input =
            json!([{"type":"text","text":"sandboxPolicy","sandboxPolicy":{"must":"remain"}}]);
        let request = json!({"id":"turn","method":"turn/start","params":{"threadId":"t","sandboxPolicy":{"type":"workspaceWrite","writableRoots":["/"]},"permissionProfile":"unsafe","approvalPolicy":"on-request","approvalsReviewer":"user","input":input,"model":"keep"}});
        let filtered = filter(request);
        assert_eq!(filtered["params"]["input"], input);
        assert_eq!(filtered["params"]["model"], "keep");
        assert_eq!(filtered["params"]["approvalPolicy"], "on-request");
        assert_eq!(filtered["params"]["approvalsReviewer"], "user");
        assert!(filtered["params"].get("sandboxPolicy").is_none());
        assert!(filtered["params"].get("permissionProfile").is_none());
        let settings = json!({"id":4,"method":"thread/settings/update","params":{"threadId":"t","collaborationMode":{"mode":"default"}}});
        assert_eq!(filter(settings.clone()), settings);
    }

    #[test]
    fn opaque_responses_and_unrelated_requests_keep_exact_bytes() {
        for bytes in [
            br#"{ "id": 3, "result":{"sandboxPolicy":"tool-result","permissions":{"read":true}} }"#
                .as_slice(),
            br#"{"method":"skills/extraRoots/set","params":{"extraRoots":["/member"]},"id":9}"#
                .as_slice(),
        ] {
            assert_eq!(locked().filter(bytes).unwrap(), bytes);
        }
    }

    #[test]
    fn protected_config_writes_and_unthreaded_execution_fail_closed() {
        for key in [
            "permissions.onehall-member",
            "\"permissions\".onehall-member",
            "features.plugins",
            "sandbox_mode",
            "profile",
            "shell_environment_policy",
            "hooks.state.fixture.trusted_hash",
        ] {
            for value in [
                json!({"id":1,"method":"config/value/write","params":{"keyPath":key,"value":"looser"}}),
                json!({"id":1,"method":"config/batchWrite","params":{"edits":[{"keyPath":key,"value":"looser"}]}}),
            ] {
                assert!(locked().filter(&value.to_string().into_bytes()).is_err());
            }
        }
        let safe = json!({"id":2,"method":"config/value/write","params":{"keyPath":"model","value":"keep"}});
        assert_eq!(filter(safe.clone()), safe);
        for method in ["command/exec", "thread/shellCommand"] {
            assert!(locked()
                .filter(
                    json!({"id":1,"method":method,"params":{}})
                        .to_string()
                        .as_bytes()
                )
                .is_err());
        }
    }

    #[test]
    fn malformed_oversized_and_unknown_config_surfaces_fail_without_echoing_contents() {
        for bytes in [
            b"private malformed fixture".to_vec(),
            vec![b'x'; MAX_LINE + 1],
            b"[]".to_vec(),
            json!({"id":1,"method":"turn/start","params":{"config":{"permissions":{}}}})
                .to_string()
                .into_bytes(),
        ] {
            assert_eq!(locked().filter(&bytes).unwrap_err(), FAILURE);
        }
        assert!(Locked::parse(&"x".repeat(MAX_CONFIG + 1)).is_err());
    }

    #[test]
    fn global_runtime_profiles_and_dotted_keys_never_enter_the_frozen_lock() {
        let mut trusted = locked().config;
        trusted.insert("features.plugins".into(), json!(true));
        trusted.insert("profile".into(), json!("unsafe"));
        trusted.insert(
            "profiles".into(),
            json!({"unsafe":{"sandbox_mode":"danger-full-access"}}),
        );
        trusted.insert("sandbox_mode".into(), json!("danger-full-access"));
        let locked = Locked::parse(&Value::Object(trusted).to_string()).unwrap();
        let mut runtime = json!({"features.plugins":true,"features.hooks":true,"profile":"unsafe","profiles":{"unsafe":{}},"sandbox_mode":"danger-full-access","sandbox_workspace_write":{"writable_roots":["/"]},"model":"keep","mcp_servers":{"codeg":{}},"features":{"plugins":true,"hooks":true}}).as_object().unwrap().clone();
        locked.apply_config(&mut runtime);
        for key in [
            "features.plugins",
            "features.hooks",
            "profile",
            "profiles",
            "sandbox_mode",
            "sandbox_workspace_write",
        ] {
            assert!(!locked.config.contains_key(key));
            assert!(!runtime.contains_key(key));
        }
        assert_eq!(runtime["features"]["plugins"], false);
        assert_eq!(runtime["features"]["hooks"], false);
        assert_eq!(runtime["model"], "keep");
        assert!(runtime.contains_key("mcp_servers"));
        let args = locked.startup_args(&["app-server".into()]).unwrap();
        assert!(!args
            .iter()
            .any(|arg| arg.to_string_lossy().contains("features.plugins=true")));
    }

    #[test]
    fn startup_flags_lock_permissions_and_disable_features_without_manual_or_key_values() {
        let args = locked()
            .startup_args(&[
                "app-server".into(),
                "-c".into(),
                "hooks={global=true}".into(),
            ])
            .unwrap();
        assert_eq!(args[0], "app-server");
        assert_eq!(args[2], "hooks={global=true}");
        let text = args
            .iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("default_permissions=\"onehall-member\""));
        assert!(!text.contains("private manual fixture"));
        assert!(!text.contains("ONEDESK_CLI_KEY"));
        for pair in args[3..].chunks(2) {
            let arg = pair[1].to_string_lossy();
            let _: toml::Value = toml::from_str(&arg).unwrap();
        }
        for args in [
            vec!["exec".into()],
            vec![
                "app-server".into(),
                "--listen".into(),
                "tcp://0.0.0.0:1234".into(),
            ],
            vec!["app-server".into(), "generate-json-schema".into()],
        ] {
            assert!(locked().startup_args(&args).is_err());
        }
    }

    #[tokio::test]
    async fn framing_rejects_unterminated_or_oversized_lines() {
        for bytes in [b"{}".to_vec(), vec![b'x'; MAX_LINE + 1]] {
            assert!(forward(bytes.as_slice(), tokio::io::sink(), &locked())
                .await
                .is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_real_child_streams_before_eof_and_is_reaped_after_eof() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "while IFS= read -r line; do printf '%s\\n' \"$line\"; done",
        ]);
        let (mut input, child_input) = tokio::io::duplex(16384);
        let (child_output, output) = tokio::io::duplex(16384);
        let task = tokio::spawn(relay(command, child_input, child_output, locked()));
        input
            .write_all(b"{\"id\":1,\"method\":\"thread/start\",\"params\":{}}\n")
            .await
            .unwrap();
        let mut output = BufReader::new(output);
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            output.read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            response["params"]["config"]["default_permissions"],
            "onehall-member"
        );
        input.shutdown().await.unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn child_exit_is_preserved_even_with_open_parent_input() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 7"]);
        let (_input, child_input) = tokio::io::duplex(128);
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            relay(command, child_input, tokio::io::sink(), locked()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(code, 7);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn protocol_error_and_ignored_eof_kill_and_reap_the_child() {
        for malformed in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let pidfile = dir.path().join("child.pid");
            let mut command = Command::new("sh");
            command
                .args(["-c", "echo $$ > \"$1\"; while :; do :; done", "fixture"])
                .arg(&pidfile);
            let (mut input, child_input) = tokio::io::duplex(128);
            let task = tokio::spawn(relay(command, child_input, tokio::io::sink(), locked()));
            let mut ready_pid = None;
            for _ in 0..100 {
                // Opening the shell redirection creates the file BEFORE echo
                // publishes its PID. File existence is not the ready signal.
                ready_pid = std::fs::read_to_string(&pidfile)
                    .ok()
                    .and_then(|text| text.trim().parse::<i32>().ok())
                    .filter(|pid| *pid > 0);
                if ready_pid.is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let pid = ready_pid.expect("fixture published a complete PID");
            if malformed {
                input.write_all(b"invalid private fixture\n").await.unwrap();
            }
            input.shutdown().await.unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.is_err(), malformed);
            // SAFETY: signal 0 only queries the fixture PID; it never signals.
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }
}

#[cfg(test)]
mod version_tests {
    use super::version_supported;

    #[test]
    fn accepts_the_verified_release_and_newer_ones() {
        assert!(version_supported("codex-cli 0.163.0\n"));
        assert!(version_supported("codex-cli 0.164.0"));
        assert!(version_supported("codex-cli 0.200.3"));
    }

    #[test]
    fn rejects_older_other_or_malformed_versions() {
        assert!(!version_supported("codex-cli 0.162.9"));
        assert!(!version_supported("codex-cli 1.0.0"));
        assert!(!version_supported("codex-cli 0.164"));
        assert!(!version_supported("codex-cli 0.164.0-alpha.1"));
        assert!(!version_supported("codex 0.164.0"));
        assert!(!version_supported(""));
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::acp::adapter_cli::BundledCli;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bridge-select-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    fn touch(path: &Path) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    fn bundled(path: &Path) -> BundledCli {
        BundledCli {
            path: path.to_path_buf(),
            version: Some("0.164.0".into()),
            needs_node: true,
        }
    }

    #[test]
    fn the_bundled_cli_is_tried_before_the_one_on_path() {
        let root = temp("order");
        let js = touch(&root.join("adapter/bin/codex.js"));
        let on_path = touch(&root.join("bin/codex"));
        let node = touch(&root.join("node"));
        let got = ordered_candidates(
            None,
            Some(&bundled(&js)),
            Some(&node),
            root.join("bin").as_os_str(),
            None,
        );
        assert_eq!(
            got,
            vec![
                Cli { program: js, runner: Some(node) },
                Cli { program: on_path, runner: None },
            ]
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn path_is_the_fallback_when_nothing_is_bundled() {
        let root = temp("fallback");
        let on_path = touch(&root.join("bin/codex"));
        let got = ordered_candidates(None, None, None, root.join("bin").as_os_str(), None);
        assert_eq!(got, vec![Cli { program: on_path, runner: None }]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_bundled_js_entry_without_node_is_skipped() {
        let root = temp("nonode");
        let js = touch(&root.join("adapter/bin/codex.js"));
        let on_path = touch(&root.join("bin/codex"));
        let got = ordered_candidates(
            None,
            Some(&bundled(&js)),
            None,
            root.join("bin").as_os_str(),
            None,
        );
        assert_eq!(got, vec![Cli { program: on_path, runner: None }]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_explicit_codex_path_is_the_only_candidate() {
        let root = temp("explicit");
        let js = touch(&root.join("adapter/bin/codex.js"));
        touch(&root.join("bin/codex"));
        let mine = touch(&root.join("mine/codex"));
        let got = ordered_candidates(
            Some(mine.to_str().unwrap()),
            Some(&bundled(&js)),
            None,
            root.join("bin").as_os_str(),
            None,
        );
        assert_eq!(got, vec![Cli { program: mine, runner: None }]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn nothing_installed_means_no_candidates() {
        let root = temp("none");
        let got = ordered_candidates(None, None, None, root.as_os_str(), None);
        assert!(got.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn launch_parts_put_the_program_behind_its_runner() {
        let program = Path::new("/a/codex.js");
        assert_eq!(
            launch_parts(Some(Path::new("/usr/bin/node")), program),
            (PathBuf::from("/usr/bin/node"), vec![OsString::from("/a/codex.js")])
        );
        assert_eq!(
            launch_parts(None, program),
            (PathBuf::from("/a/codex.js"), Vec::new())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_version_check_runs_a_script_through_its_runner() {
        let root = temp("runner");
        let script = touch(&root.join("codex.js"));
        std::fs::write(&script, "echo 'codex-cli 0.164.0'\n").unwrap();
        let sh = PathBuf::from("/bin/sh");
        let ok = Cli { program: script.clone(), runner: Some(sh.clone()) };
        assert!(verify_version(&ok).await.is_ok());

        std::fs::write(&script, "echo 'codex-cli 0.162.0'\n").unwrap();
        assert!(verify_version(&Cli { program: script, runner: Some(sh) })
            .await
            .is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
