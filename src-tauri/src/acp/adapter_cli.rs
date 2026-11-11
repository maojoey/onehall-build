//! Adapter → CLI resolution shared by ordinary ACP and Hall launches.
//!
//! `codex-acp` and `claude-agent-acp` each carry the vendor CLI they were
//! built against (`@openai/codex`, the `@anthropic-ai/claude-agent-sdk-*`
//! platform package). That CLI, not whatever `codex` / `claude` happens to be
//! on PATH, is the one the adapter is compatible with, so callers ask here
//! first and only fall back to PATH when nothing is bundled.
//!
//! Everything below is plain filesystem inspection (no subprocess, no node):
//! it mirrors node's "nearest `node_modules` first" lookup so the answer
//! matches what the adapter itself would load.

use std::path::{Path, PathBuf};

/// The vendor CLI an adapter ships with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledCli {
    /// Executable (Claude) or node entry script (Codex, `needs_node`).
    pub path: PathBuf,
    /// `version` from the CLI package's own `package.json`.
    pub version: Option<String>,
    /// `true` when `path` is a `.js` entry that has to be run through node.
    pub needs_node: bool,
}

/// One installed adapter package and the CLI beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterRuntime {
    pub package_dir: PathBuf,
    pub adapter_version: Option<String>,
    pub cli: Option<BundledCli>,
}

/// Which adapter package to inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    Codex,
    Claude,
}

impl AdapterKind {
    /// Directory name under the `@agentclientprotocol` scope. It is also the
    /// command the package installs.
    pub fn package_name(self) -> &'static str {
        match self {
            AdapterKind::Codex => "codex-acp",
            AdapterKind::Claude => "claude-agent-acp",
        }
    }

    /// Full npm package name.
    pub fn npm_package(self) -> String {
        format!("@agentclientprotocol/{}", self.package_name())
    }

    /// The adapter an agent launches, if it launches one.
    pub fn for_agent(agent_type: crate::models::agent::AgentType) -> Option<Self> {
        use crate::models::agent::AgentType;
        match agent_type {
            AgentType::Codex => Some(AdapterKind::Codex),
            AgentType::ClaudeCode => Some(AdapterKind::Claude),
            _ => None,
        }
    }

    /// The adapter that installs the command `cmd`, if any.
    pub fn for_command(cmd: &str) -> Option<Self> {
        [AdapterKind::Codex, AdapterKind::Claude]
            .into_iter()
            .find(|kind| kind.package_name() == cmd)
    }
}

/// The `node_modules` directory of an npm prefix (`-g` layout differs by OS).
pub fn global_node_modules(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.join("node_modules")
    } else {
        prefix.join("lib").join("node_modules")
    }
}

/// Where an adapter package lives under a `node_modules` directory.
pub fn adapter_package_dir(node_modules: &Path, kind: AdapterKind) -> PathBuf {
    node_modules
        .join("@agentclientprotocol")
        .join(kind.package_name())
}

/// `version` of the package rooted at `dir`, if it has a readable manifest.
pub fn read_package_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let version = value.get("version")?.as_str()?.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// Find `<package>` the way node resolves it from `start`: in `start`'s own
/// `node_modules`, then in each ancestor's. An ancestor that is itself a
/// `node_modules` directory is searched directly (hoisted installs).
pub fn find_nested_package(start: &Path, package: &str) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let direct = dir.join("node_modules").join(package);
        if direct.join("package.json").is_file() {
            return Some(direct);
        }
        if dir.file_name().is_some_and(|name| name == "node_modules") {
            let hoisted = dir.join(package);
            if hoisted.join("package.json").is_file() {
                return Some(hoisted);
            }
        }
        None
    })
}

fn codex_cli(package_dir: &Path) -> Option<BundledCli> {
    let cli_dir = find_nested_package(package_dir, "@openai/codex")?;
    let path = cli_dir.join("bin").join("codex.js");
    path.is_file().then(|| BundledCli {
        path,
        version: read_package_version(&cli_dir),
        needs_node: true,
    })
}

/// Platform package suffix the Claude SDK ships binaries under.
fn claude_platform_prefix() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("claude-agent-sdk-{os}-{arch}")
}

fn claude_cli(package_dir: &Path) -> Option<BundledCli> {
    let sdk_dir = find_nested_package(package_dir, "@anthropic-ai/claude-agent-sdk")?;
    let scope = sdk_dir.parent()?;
    let wanted = claude_platform_prefix();
    let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
    let mut platform_dirs: Vec<PathBuf> = std::fs::read_dir(scope)
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                // `-musl` builds only fit musl hosts; the plain build comes first.
                .is_some_and(|n| n == wanted || n.starts_with(&format!("{wanted}-")))
        })
        .collect();
    platform_dirs.sort();
    platform_dirs.into_iter().find_map(|dir| {
        let path = dir.join(exe);
        path.is_file().then(|| BundledCli {
            path,
            version: read_package_version(&dir),
            needs_node: false,
        })
    })
}

/// Inspect one adapter package directory. `None` when it is not installed.
pub fn inspect_adapter(package_dir: &Path, kind: AdapterKind) -> Option<AdapterRuntime> {
    if !package_dir.join("package.json").is_file() {
        return None;
    }
    let cli = match kind {
        AdapterKind::Codex => codex_cli(package_dir),
        AdapterKind::Claude => claude_cli(package_dir),
    };
    Some(AdapterRuntime {
        package_dir: package_dir.to_path_buf(),
        adapter_version: read_package_version(package_dir),
        cli,
    })
}

/// Adapter package directories to look in, in launch order: a managed version
/// (see [`crate::acp::adapter_store`]), the npm global prefix, then OneHall's
/// user prefix (`~/.codeg/npm-global`, used when a global install hit EACCES).
pub async fn candidate_package_dirs(kind: AdapterKind) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(managed) = crate::acp::adapter_store::active_package_dir(
        &crate::acp::adapter_store::default_root(),
        kind,
    ) {
        dirs.push(managed);
    }
    if let Some(prefix) = crate::commands::acp::cached_npm_global_prefix().await {
        dirs.push(adapter_package_dir(&global_node_modules(&prefix), kind));
    }
    if let Some(prefix) = crate::process::user_npm_prefix() {
        dirs.push(adapter_package_dir(&global_node_modules(&prefix), kind));
    }
    dirs.dedup();
    dirs
}

/// The first installed adapter of `kind`, with its bundled CLI.
pub async fn installed_adapter(kind: AdapterKind) -> Option<AdapterRuntime> {
    candidate_package_dirs(kind)
        .await
        .iter()
        .find_map(|dir| inspect_adapter(dir, kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn package(dir: &Path, version: &str) {
        write(
            &dir.join("package.json"),
            &format!(r#"{{"version":"{version}"}}"#),
        );
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adapter-cli-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nested_cli_wins_over_a_hoisted_one() {
        let root = temp("nested");
        let modules = root.join("node_modules");
        let adapter = adapter_package_dir(&modules, AdapterKind::Codex);
        package(&adapter, "2.3.0");
        // Hoisted copy (older) and the copy nested under the adapter (newer).
        package(&modules.join("@openai/codex"), "0.150.0");
        write(&modules.join("@openai/codex/bin/codex.js"), "");
        package(&adapter.join("node_modules/@openai/codex"), "0.164.0");
        write(&adapter.join("node_modules/@openai/codex/bin/codex.js"), "");

        let runtime = inspect_adapter(&adapter, AdapterKind::Codex).unwrap();
        assert_eq!(runtime.adapter_version.as_deref(), Some("2.3.0"));
        let cli = runtime.cli.unwrap();
        assert_eq!(cli.version.as_deref(), Some("0.164.0"));
        assert!(cli.needs_node);
        assert!(cli.path.starts_with(&adapter));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hoisted_cli_is_found_when_nothing_is_nested() {
        let root = temp("hoisted");
        let modules = root.join("node_modules");
        let adapter = adapter_package_dir(&modules, AdapterKind::Codex);
        package(&adapter, "2.3.0");
        package(&modules.join("@openai/codex"), "0.164.0");
        write(&modules.join("@openai/codex/bin/codex.js"), "");

        let cli = inspect_adapter(&adapter, AdapterKind::Codex)
            .unwrap()
            .cli
            .unwrap();
        assert_eq!(cli.version.as_deref(), Some("0.164.0"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn adapter_without_a_cli_reports_none() {
        let root = temp("nocli");
        let adapter = adapter_package_dir(&root.join("node_modules"), AdapterKind::Codex);
        package(&adapter, "2.3.0");
        let runtime = inspect_adapter(&adapter, AdapterKind::Codex).unwrap();
        assert_eq!(runtime.cli, None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_adapter_is_none() {
        let root = temp("missing");
        assert_eq!(inspect_adapter(&root.join("nope"), AdapterKind::Codex), None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn claude_binary_comes_from_the_matching_platform_package() {
        let root = temp("claude");
        let modules = root.join("node_modules");
        let adapter = adapter_package_dir(&modules, AdapterKind::Claude);
        package(&adapter, "0.99.0");
        let scope = adapter.join("node_modules/@anthropic-ai");
        package(&scope.join("claude-agent-sdk"), "0.3.312");
        let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
        let mine = scope.join(claude_platform_prefix());
        package(&mine, "2.1.312");
        write(&mine.join(exe), "");
        // A package for another platform must never be picked.
        let other = scope.join("claude-agent-sdk-plan9-mips");
        package(&other, "9.9.9");
        write(&other.join(exe), "");

        let cli = inspect_adapter(&adapter, AdapterKind::Claude)
            .unwrap()
            .cli
            .unwrap();
        assert_eq!(cli.version.as_deref(), Some("2.1.312"));
        assert!(!cli.needs_node);
        assert!(cli.path.starts_with(&mine));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unreadable_or_blank_manifests_have_no_version() {
        let root = temp("manifest");
        write(&root.join("package.json"), "not json");
        assert_eq!(read_package_version(&root), None);
        write(&root.join("package.json"), r#"{"version":"  "}"#);
        assert_eq!(read_package_version(&root), None);
        fs::remove_dir_all(root).ok();
    }
}
