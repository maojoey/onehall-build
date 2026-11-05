//! What a team launch hands one member's agent process, besides the prompt:
//! the role manual, engine-native limits derived from the member's declared
//! scope, and its own read-only OneDesk key. This is the in-hall equivalent
//! of the assistants' old `scripts/codex-start.py`.
//!
//! A restricted Codex member reads `CODEX_CONFIG` (JSON, merged into every
//! thread's config) via codex-acp. Claude Code reads its role manual
//! (`CLAUDE.md`) on its own and keeps asking for edits and commands, so it only
//! gets the key. A local assistant gets neither the frozen Codex config nor the
//! key: `execution_policy` decides which of these applies.
use super::config::{read::Root, LaunchProfile, MemberAccess};
use super::execution_policy::{self, Decision, Platform, MODE_ENV};
use crate::acp::host_tools_policy::{HOST_TOOLS_AGENT, HOST_TOOLS_ENV};
use crate::app_error::AppCommandError;
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) const CODEX_CONFIG: &str = "CODEX_CONFIG";
const PROFILE: &str = "onehall-member";
const MAX_MANUAL_BYTES: u64 = 256 * 1024;
const MAX_KEY_BYTES: u64 = 4 * 1024;

fn invalid(message: &str) -> AppCommandError {
    AppCommandError::invalid_input(message.to_owned())
}

/// Environment for one member's agent process. Values may be secret; callers
/// must never log them.
pub(crate) async fn launch_env(
    profile: &LaunchProfile,
) -> Result<BTreeMap<String, String>, AppCommandError> {
    // Refuse before any file is read or any process exists.
    let decision = execution_policy::decide(profile, Platform::current())?;
    let profile = profile.clone();
    tokio::task::spawn_blocking(move || launch_env_for(&profile, &decision))
        .await
        .map_err(|_| invalid("Member launch preparation failed"))?
}

#[cfg(test)]
fn launch_env_blocking(p: &LaunchProfile) -> Result<BTreeMap<String, String>, AppCommandError> {
    launch_env_for(p, &execution_policy::decide(p, Platform::current())?)
}

fn launch_env_for(
    p: &LaunchProfile,
    decision: &Decision,
) -> Result<BTreeMap<String, String>, AppCommandError> {
    let cwd = PathBuf::from(&p.cwd);
    let root = Root::open(&cwd).map_err(|_| invalid("Member directory is unavailable"))?;
    // An explicit empty override also blocks an agent setting or server env
    // from lending another member's key to a no-key launch.
    let mut env = BTreeMap::from([
        ("ONEDESK_CLI_KEY".into(), String::new()),
        (MODE_ENV.into(), execution_policy::marker(decision).into()),
    ]);
    if decision.key && p.access.onedesk_key == "read" {
        let raw = root
            .read_fixed(".onedesk/cli-key", MAX_KEY_BYTES)
            .map_err(|_| invalid("The member's .onedesk/cli-key cannot be read safely"))?
            .ok_or_else(|| {
                invalid("This member declares onedesk_key: read but has no .onedesk/cli-key")
            })?;
        let raw = String::from_utf8(raw)
            .map_err(|_| invalid("The member's .onedesk/cli-key is not a single key"))?;
        let key = raw.trim();
        if key.is_empty() || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(invalid("The member's .onedesk/cli-key is not a single key"));
        }
        env.insert("ONEDESK_CLI_KEY".into(), key.to_owned());
        env.insert(
            "ONEDESK_URL".into(),
            std::env::var("ONEHALL_ONEDESK_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:8420".into()),
        );
    }
    if decision.bridge {
        let config = codex_config(&cwd, &p.access, &runtime_read_roots())?;
        env.insert(CODEX_CONFIG.into(), config.to_string());
        // Both ACP execution channels must remain inside the native sandbox.
        // Runtime settings are merged first, so this also overrides `default`.
        env.insert(HOST_TOOLS_ENV.into(), HOST_TOOLS_AGENT.into());
    }
    Ok(env)
}

/// Codex's view of a member: a role manual when the directory only has
/// `CLAUDE.md` (Codex reads `AGENTS.md` itself), and a permission profile in
/// which the machine is denied, the member's declared read / write rules are
/// granted, its key directory is never readable, and the network matches
/// `scope.network`.
pub(crate) fn codex_config(
    cwd: &Path,
    access: &MemberAccess,
    runtime_roots: &[PathBuf],
) -> Result<Value, AppCommandError> {
    let root = Root::open(cwd).map_err(|_| invalid("Member directory is unavailable"))?;
    let mut fs = Map::new();
    fs.insert(":root".into(), json!("deny"));
    fs.insert(":minimal".into(), json!("read"));
    for root in runtime_roots {
        fs.insert(root.to_string_lossy().into_owned(), json!("read"));
    }
    for rule in &access.read {
        fs.insert(path_of(cwd, &root, rule)?, json!("read"));
    }
    for rule in &access.write {
        fs.insert(path_of(cwd, &root, rule)?, json!("write"));
    }
    fs.insert(
        cwd.join(".onedesk").to_string_lossy().into_owned(),
        json!("deny"),
    );
    let (enabled, domains) = match access.network.as_str() {
        "local" => (true, json!({"127.0.0.1": "allow", "localhost": "allow"})),
        "default" => (true, json!({"*": "allow"})),
        _ => (false, json!({})),
    };
    let mut config = json!({
        "default_permissions": PROFILE,
        "permissions": {PROFILE: {"filesystem": fs, "network": {"enabled": enabled, "domains": domains}}},
        "features": {
            "network_proxy": enabled, "shell_snapshot": false,
            "shell_snapshot_v2": false, "plugins": false, "apps": false,
            "hooks": false, "multi_agent": false, "multi_agent_v2": false,
            "remote_plugin": false, "plugin_sharing": false,
        },
        "hooks": {},
        "apps": {"_default": {"enabled": false}},
        "notify": [],
        "allow_login_shell": false,
        "shell_environment_policy": shell_environment_policy(access),
        "web_search": if access.network == "default" { "live" } else { "disabled" },
    });
    if let Some(manual) = role_manual(&root)? {
        config["developer_instructions"] = json!(manual);
    }
    Ok(config)
}

/// Start with process variables, then keep an explicit core allowlist. Using
/// `core` alone would drop the member key before `include_only` runs. Default
/// key exclusions must be disabled for this one named key; the allowlist still
/// excludes every other secret (including the server's CODEG_TOKEN).
fn shell_environment_policy(access: &MemberAccess) -> Value {
    let mut names = vec![
        "PATH", "SHELL", "TMPDIR", "TEMP", "TMP", "HOME", "LANG", "LC_ALL", "LC_CTYPE", "LOGNAME",
        "USER",
    ];
    #[cfg(windows)]
    names.extend([
        "PATHEXT",
        "COMSPEC",
        "SYSTEMROOT",
        "WINDIR",
        "SYSTEMDRIVE",
        "USERNAME",
        "USERDOMAIN",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
        "PROGRAMDATA",
        "LOCALAPPDATA",
        "APPDATA",
        "POWERSHELL",
        "PWSH",
    ]);
    if access.onedesk_key == "read" {
        names.extend(["ONEDESK_CLI_KEY", "ONEDESK_URL"]);
    }
    json!({"inherit": "all", "ignore_default_excludes": true, "include_only": names})
}

/// `CLAUDE.md` for a Codex member whose directory has no `AGENTS.md`.
fn role_manual(root: &Root) -> Result<Option<String>, AppCommandError> {
    let read = |name| {
        root.read_fixed(name, MAX_MANUAL_BYTES)
            .map_err(|_| invalid("The member's role manual cannot be read safely"))
    };
    if read("AGENTS.md")?.is_some() {
        return Ok(None);
    }
    read("CLAUDE.md")?
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|_| invalid("The member's CLAUDE.md is not a readable manual"))
        })
        .transpose()
}

/// Native path grants cover entire directory trees. Only recursive declarations
/// have equivalent semantics; reject every other pattern instead of widening it.
/// This lexical validation also runs before a Codex candidate can be frozen.
pub(super) fn scope_directory(rule: &str) -> Result<String, AppCommandError> {
    if rule == "**" {
        return Ok(String::new());
    }
    let prefix = rule
        .strip_suffix("/**")
        .ok_or_else(|| invalid("Codex members need recursive scope rules such as notes/**"))?;
    if prefix.is_empty()
        || prefix.starts_with(['/', '~', '!'])
        || prefix.trim() != prefix
        || prefix.chars().any(char::is_control)
        || prefix.contains(['*', '?', '[', ']', '{', '}', '\\', ':'])
        || prefix.split('/').any(|part| part == "..")
    {
        return Err(invalid("Codex scope rule cannot be represented safely"));
    }
    let parts: Vec<_> = prefix
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() || parts[0] == ".onedesk" {
        return Err(invalid(
            "Codex scope cannot grant the member's key directory",
        ));
    }
    Ok(parts.join("/"))
}

fn path_of(cwd: &Path, root: &Root, rule: &str) -> Result<String, AppCommandError> {
    let relative = scope_directory(rule)?;
    root.check_directory(&relative)
        .map_err(|_| invalid("Codex scope root is not a safe member directory"))?;
    Ok(if relative.is_empty() {
        cwd.to_owned()
    } else {
        cwd.join(relative)
    }
    .to_string_lossy()
    .into_owned())
}

/// Directories the agent's own tools must read even under `:root = deny`: the
/// Node install (for `node`-based CLIs) and the npm-installed Codex, plus any
/// in `ONEHALL_MEMBER_READ_ROOTS` (colon-separated).
fn runtime_read_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(node) = which("node").and_then(|p| std::fs::canonicalize(p).ok()) {
        if let Some(root) = node.parent().and_then(Path::parent) {
            roots.push(root.to_path_buf());
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let codex = PathBuf::from(home).join(".local/lib/codex-npm");
        if codex.is_dir() {
            roots.push(codex);
        }
    }
    if let Some(extra) = std::env::var_os("ONEHALL_MEMBER_READ_ROOTS") {
        roots.extend(std::env::split_paths(&extra).filter(|p| p.is_absolute()));
    }
    roots
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Merges a member's `CODEX_CONFIG` over one already in the agent settings,
/// key by key at the top level; every other variable is replaced.
pub(crate) fn merge_into(runtime: &mut BTreeMap<String, String>, member: BTreeMap<String, String>) {
    for (key, value) in member {
        if key == CODEX_CONFIG {
            if let (Some(Ok(Value::Object(mut base))), Ok(Value::Object(ours))) = (
                runtime.get(&key).map(|v| serde_json::from_str::<Value>(v)),
                serde_json::from_str::<Value>(&value),
            ) {
                base.extend(ours);
                runtime.insert(key, Value::Object(base).to_string());
                continue;
            }
        }
        runtime.insert(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Production registration requires a canonical directory. macOS's usual
    // temporary root contains a /var alias, so fixtures must satisfy that same
    // contract instead of failing before exercising the launch behavior.
    fn directory() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
    }

    fn access(read: &[&str], write: &[&str], network: &str, key: &str) -> MemberAccess {
        MemberAccess {
            read: read.iter().map(|s| s.to_string()).collect(),
            write: write.iter().map(|s| s.to_string()).collect(),
            network: network.into(),
            onedesk_key: key.into(),
        }
    }

    #[test]
    fn codex_gets_the_manual_and_only_the_declared_reach() {
        let dir = directory();
        std::fs::write(dir.path().join("CLAUDE.md"), "# 研究生培养助手").unwrap();
        let roots = vec![PathBuf::from("/opt/node")];
        let c = codex_config(
            dir.path(),
            &access(&["**"], &["memory/**"], "local", "read"),
            &roots,
        )
        .unwrap();
        assert_eq!(c["developer_instructions"], "# 研究生培养助手");
        assert_eq!(c["default_permissions"], PROFILE);
        let fs = &c["permissions"][PROFILE]["filesystem"];
        let at = |p: &Path| fs[p.to_string_lossy().as_ref()].clone();
        assert_eq!(fs[":root"], "deny");
        assert_eq!(fs["/opt/node"], "read");
        assert_eq!(at(dir.path()), "read");
        assert_eq!(at(&dir.path().join("memory")), "write");
        assert_eq!(at(&dir.path().join(".onedesk")), "deny");
        let net = &c["permissions"][PROFILE]["network"];
        assert_eq!(net["enabled"], true);
        assert_eq!(net["domains"]["127.0.0.1"], "allow");
        assert!(net["domains"].get("*").is_none());
        assert_eq!(c["web_search"], "disabled");
    }

    #[test]
    fn agents_md_wins_and_network_none_is_off() {
        let dir = directory();
        std::fs::write(dir.path().join("CLAUDE.md"), "claude").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "agents").unwrap();
        let c = codex_config(dir.path(), &access(&["**"], &[], "none", "none"), &[]).unwrap();
        assert!(c.get("developer_instructions").is_none());
        assert_eq!(c["permissions"][PROFILE]["network"]["enabled"], false);
        assert_eq!(c["features"]["network_proxy"], false);
    }

    #[test]
    fn unsupported_scopes_fail_closed_for_both_read_and_write() {
        let dir = directory();
        for rule in [
            "notes/*.md",
            "*.md",
            "docs/[ab]?.md",
            "src/*.{rs,ts}",
            "reports",
            "a/**/**",
            ".onedesk/**",
            ".onedesk/cli-key/**",
            "./.onedesk/private/**",
            "a/../../outside/**",
        ] {
            for policy in [
                access(&[rule], &[], "none", "none"),
                access(&["**"], &[rule], "none", "none"),
            ] {
                assert!(codex_config(dir.path(), &policy, &[]).is_err(), "{rule}");
            }
        }
        let c = codex_config(
            dir.path(),
            &access(&["notes/**"], &["reports/**", "**"], "default", "none"),
            &[],
        )
        .unwrap();
        let fs = &c["permissions"][PROFILE]["filesystem"];
        assert_eq!(
            fs[dir.path().join("notes").to_string_lossy().as_ref()],
            "read"
        );
        assert_eq!(
            fs[dir.path().join("reports").to_string_lossy().as_ref()],
            "write"
        );
        assert_eq!(fs[dir.path().to_string_lossy().as_ref()], "write");
        assert_eq!(
            fs[dir.path().join(".onedesk").to_string_lossy().as_ref()],
            "deny"
        );
        assert_eq!(c["web_search"], "live");
    }

    #[test]
    fn the_key_comes_from_the_members_own_file_and_never_from_elsewhere() {
        let dir = directory();
        let mut p = LaunchProfile {
            member_id: "m".into(),
            engine: "claude_code".into(),
            member_revision: 1,
            cwd: dir.path().to_string_lossy().into_owned(),
            assistant_uid: None,
            mode: "default".into(),
            config: BTreeMap::new(),
            brief: String::new(),
            access: access(&["**"], &[], "local", "read"),
            execution_mode: None,
            capabilities: None,
        };
        assert!(launch_env_blocking(&p).is_err());
        std::fs::create_dir(dir.path().join(".onedesk")).unwrap();
        std::fs::write(dir.path().join(".onedesk/cli-key"), "secret-key\n").unwrap();
        let env = launch_env_blocking(&p).unwrap();
        assert_eq!(env["ONEDESK_CLI_KEY"], "secret-key");
        assert!(env.contains_key("ONEDESK_URL"));
        // Claude Code reads CLAUDE.md itself and asks before edits.
        assert!(!env.contains_key(CODEX_CONFIG));
        p.access.onedesk_key = "none".into();
        p.engine = "codex".into();
        let env = launch_env_blocking(&p).unwrap();
        assert_eq!(env["ONEDESK_CLI_KEY"], "");
        assert!(env.contains_key(CODEX_CONFIG));
    }

    fn profile(cwd: &Path) -> LaunchProfile {
        LaunchProfile {
            member_id: "fixture".into(),
            engine: "codex".into(),
            member_revision: 1,
            cwd: cwd.to_string_lossy().into_owned(),
            assistant_uid: None,
            mode: "read-only".into(),
            config: BTreeMap::new(),
            brief: String::new(),
            access: access(&["**"], &[], "none", "read"),
            execution_mode: None,
            capabilities: None,
        }
    }

    #[test]
    fn key_and_manual_reads_are_bounded_regular_files() {
        let dir = directory();
        let p = profile(dir.path());
        std::fs::create_dir(dir.path().join(".onedesk")).unwrap();
        let key = dir.path().join(".onedesk/cli-key");
        let file = std::fs::File::create(&key).unwrap();
        file.set_len(MAX_KEY_BYTES + 1).unwrap();
        assert!(launch_env_blocking(&p).is_err());
        std::fs::remove_file(&key).unwrap();
        std::fs::create_dir(&key).unwrap();
        assert!(launch_env_blocking(&p).is_err());
        std::fs::remove_dir(&key).unwrap();
        std::fs::write(key, "fixture-key").unwrap();
        let manual = dir.path().join("CLAUDE.md");
        let file = std::fs::File::create(&manual).unwrap();
        file.set_len(MAX_MANUAL_BYTES + 1).unwrap();
        assert!(launch_env_blocking(&p).is_err());
        std::fs::remove_file(&manual).unwrap();
        std::fs::create_dir(&manual).unwrap();
        assert!(launch_env_blocking(&p).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_redirect_keys_manuals_or_scope_roots() {
        use std::os::unix::fs::symlink;
        let dir = directory();
        let outside = directory();
        std::fs::write(outside.path().join("cli-key"), "other-member-fixture").unwrap();
        std::fs::write(outside.path().join("manual"), "other-member-manual").unwrap();
        let mut p = profile(dir.path());
        symlink(outside.path(), dir.path().join(".onedesk")).unwrap();
        assert!(launch_env_blocking(&p).is_err());
        std::fs::remove_file(dir.path().join(".onedesk")).unwrap();
        std::fs::create_dir(dir.path().join(".onedesk")).unwrap();
        symlink(
            outside.path().join("cli-key"),
            dir.path().join(".onedesk/cli-key"),
        )
        .unwrap();
        assert!(launch_env_blocking(&p).is_err());
        p.access.onedesk_key = "none".into();
        for name in ["CLAUDE.md", "AGENTS.md"] {
            symlink(outside.path().join("manual"), dir.path().join(name)).unwrap();
            assert!(launch_env_blocking(&p).is_err());
            std::fs::remove_file(dir.path().join(name)).unwrap();
        }
        symlink(outside.path(), dir.path().join("notes")).unwrap();
        for policy in [
            access(&["notes/**"], &[], "none", "none"),
            access(&["**"], &["notes/out/**"], "none", "none"),
        ] {
            assert!(codex_config(dir.path(), &policy, &[]).is_err());
        }
        symlink(dir.path(), outside.path().join("linked-member")).unwrap();
        p.cwd = outside
            .path()
            .join("linked-member")
            .to_string_lossy()
            .into_owned();
        assert!(launch_env_blocking(&p).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn key_and_manual_fifos_are_refused_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let dir = directory();
        std::fs::create_dir(dir.path().join(".onedesk")).unwrap();
        let mut p = profile(dir.path());
        for name in [".onedesk/cli-key", "CLAUDE.md", "AGENTS.md"] {
            let path = dir.path().join(name);
            let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: fixture path is a valid, live CString; mode is private.
            assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
            assert!(launch_env_blocking(&p).is_err());
            std::fs::remove_file(path).unwrap();
            p.access.onedesk_key = "none".into();
        }
    }

    #[tokio::test]
    async fn member_environment_overrides_hosted_tools_settings_and_foreign_keys() {
        use crate::acp::host_tools_policy::HostToolsPolicy;
        let dir = directory();
        let mut p = profile(dir.path());
        p.access.onedesk_key = "none".into();
        let member_env = launch_env(&p).await.unwrap();
        let mut runtime = BTreeMap::from([
            (HOST_TOOLS_ENV.into(), "default".into()),
            ("ONEDESK_CLI_KEY".into(), "foreign-fixture-key".into()),
        ]);
        merge_into(&mut runtime, member_env);
        assert_eq!(HostToolsPolicy::from_env(&runtime), HostToolsPolicy::Agent);
        assert!(!HostToolsPolicy::from_env(&runtime).hosts_channels());
        assert_eq!(runtime["ONEDESK_CLI_KEY"], "");
        let config: Value = serde_json::from_str(&runtime[CODEX_CONFIG]).unwrap();
        assert_eq!(config["allow_login_shell"], false);
        assert_eq!(config["features"]["shell_snapshot"], false);
        let policy = &config["shell_environment_policy"];
        assert_eq!(policy["inherit"], "all");
        assert_eq!(policy["ignore_default_excludes"], true);
        let names = policy["include_only"].as_array().unwrap();
        assert!(names.contains(&json!("PATH")));
        for secret in ["ONEDESK_CLI_KEY", "CODEG_TOKEN", "OPENAI_API_KEY"] {
            assert!(!names.contains(&json!(secret)));
        }
        let policy = shell_environment_policy(&access(&["**"], &[], "local", "read"));
        let names = policy["include_only"].as_array().unwrap();
        assert!(names.contains(&json!("ONEDESK_CLI_KEY")));
        assert!(names.contains(&json!("ONEDESK_URL")));
        assert!(!names.contains(&json!("CODEG_TOKEN")));
        assert!(policy.get("set").is_none());
    }

    #[test]
    fn member_codex_config_merges_over_the_agent_setting() {
        let mut runtime = BTreeMap::from([
            ("CODEX_PATH".to_string(), "/bin/codex".to_string()),
            (
                CODEX_CONFIG.to_string(),
                r#"{"hooks":{"a":1},"web_search":"live"}"#.to_string(),
            ),
        ]);
        merge_into(
            &mut runtime,
            BTreeMap::from([(
                CODEX_CONFIG.to_string(),
                r#"{"web_search":"disabled"}"#.to_string(),
            )]),
        );
        let merged: Value = serde_json::from_str(&runtime[CODEX_CONFIG]).unwrap();
        assert_eq!(merged["hooks"]["a"], 1);
        assert_eq!(merged["web_search"], "disabled");
        assert_eq!(runtime["CODEX_PATH"], "/bin/codex");
    }
}
