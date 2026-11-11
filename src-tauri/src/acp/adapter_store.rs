//! Versioned, probe-gated installs of the ACP adapters.
//!
//! Each version lives in its own directory,
//! `<root>/<adapter>/<version>/node_modules/...`, installed with
//! `npm install --prefix`. `state.json` names the `current` and `previous`
//! version per adapter. An upgrade installs the new version beside the old one,
//! runs the handshake probe, and only then moves `current`; a failure deletes
//! the new directory and leaves everything else as it was. Rolling back swaps
//! the two pointers.
//!
//! When an adapter has a `current` here, launches use it instead of a global
//! npm copy. The version number is never pinned: any version that passes the
//! probe is accepted.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::acp::adapter_cli::{
    adapter_package_dir, inspect_adapter, AdapterKind, AdapterRuntime,
};
use crate::acp::adapter_probe::ProbeReport;

const STATE_FILE: &str = "state.json";
/// The adapter plus its bundled CLI is well over 100 MB; a slow link needs room.
const NPM_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const NPM_ERROR_TAIL: usize = 800;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("invalid adapter version: {0}")]
    InvalidVersion(String),
    #[error("could not look up the latest version: {0}")]
    Lookup(String),
    #[error("install failed: {0}")]
    Install(String),
    #[error("{version} did not pass the startup check, so {kept} stays active: {detail}")]
    ProbeFailed {
        version: String,
        kept: String,
        detail: String,
    },
    #[error("there is no previous version to roll back to")]
    NoPrevious,
    #[error("{0}")]
    Io(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pointers {
    pub current: Option<String>,
    pub previous: Option<String>,
}

type State = BTreeMap<String, Pointers>;

/// Everything the settings page shows about one adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdapterStatus {
    /// `true` when launches use a version from this store.
    pub managed: bool,
    pub adapter_version: Option<String>,
    pub cli_version: Option<String>,
    pub current: Option<String>,
    pub previous: Option<String>,
    pub installed_versions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpgradeOutcome {
    pub version: String,
    pub previous: Option<String>,
    pub probe: ProbeReport,
}

pub fn default_root() -> PathBuf {
    crate::paths::codeg_home_dir().join("adapters")
}

fn version_dir(root: &Path, kind: AdapterKind, version: &str) -> PathBuf {
    root.join(kind.package_name()).join(version)
}

fn package_dir_of(root: &Path, kind: AdapterKind, version: &str) -> PathBuf {
    adapter_package_dir(&version_dir(root, kind, version).join("node_modules"), kind)
}

fn is_installed(root: &Path, kind: AdapterKind, version: &str) -> bool {
    package_dir_of(root, kind, version)
        .join("package.json")
        .is_file()
}

fn load(root: &Path) -> State {
    std::fs::read_to_string(root.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(root: &Path, state: &State) -> Result<(), StoreError> {
    let io = |e: std::io::Error| StoreError::Io(e.to_string());
    std::fs::create_dir_all(root).map_err(io)?;
    let temp = root.join(format!("{STATE_FILE}.tmp"));
    let text = serde_json::to_string_pretty(state).map_err(|e| StoreError::Io(e.to_string()))?;
    std::fs::write(&temp, text).map_err(io)?;
    std::fs::rename(&temp, root.join(STATE_FILE)).map_err(io)
}

/// The pointers whose directories still exist; a half-deleted version never
/// counts as active.
pub fn pointers(root: &Path, kind: AdapterKind) -> Pointers {
    let mut stored = load(root).remove(kind.package_name()).unwrap_or_default();
    let valid = |version: &Option<String>| {
        version
            .as_deref()
            .is_some_and(|v| is_installed(root, kind, v))
    };
    if !valid(&stored.current) {
        stored.current = None;
    }
    if !valid(&stored.previous) {
        stored.previous = None;
    }
    stored
}

pub fn installed_versions(root: &Path, kind: AdapterKind) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(kind.package_name())) else {
        return Vec::new();
    };
    let mut versions: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.ends_with(".partial") && is_installed(root, kind, name))
        .collect();
    versions.sort();
    versions
}

/// The adapter package a launch should use, if a managed version is active.
pub fn active_package_dir(root: &Path, kind: AdapterKind) -> Option<PathBuf> {
    let current = pointers(root, kind).current?;
    Some(package_dir_of(root, kind, &current))
}

/// The command a launch should run for the managed adapter, if one is active.
pub fn managed_command(root: &Path, kind: AdapterKind) -> Option<PathBuf> {
    let current = pointers(root, kind).current?;
    let bin = version_dir(root, kind, &current)
        .join("node_modules")
        .join(".bin");
    let name = kind.package_name();
    let candidates: &[String] = &[
        #[cfg(windows)]
        format!("{name}.cmd"),
        name.to_string(),
    ];
    candidates
        .iter()
        .map(|file| bin.join(file))
        .find(|path| path.is_file())
}

/// Make `version` current, remembering the version it replaces.
pub fn activate(root: &Path, kind: AdapterKind, version: &str) -> Result<(), StoreError> {
    let mut state = load(root);
    let mut entry = pointers(root, kind);
    if entry.current.as_deref() == Some(version) {
        return Ok(());
    }
    entry.previous = entry.current.take();
    entry.current = Some(version.to_string());
    state.insert(kind.package_name().to_string(), entry);
    save(root, &state)
}

/// Swap `current` and `previous`; returns the version now current.
pub fn rollback(root: &Path, kind: AdapterKind) -> Result<String, StoreError> {
    let entry = pointers(root, kind);
    let (Some(previous), current) = (entry.previous, entry.current) else {
        return Err(StoreError::NoPrevious);
    };
    let mut state = load(root);
    state.insert(
        kind.package_name().to_string(),
        Pointers { current: Some(previous.clone()), previous: current },
    );
    save(root, &state)?;
    Ok(previous)
}

/// Delete every installed version that is neither current nor previous, and
/// any interrupted install.
pub fn prune(root: &Path, kind: AdapterKind) {
    let keep = pointers(root, kind);
    let Ok(entries) = std::fs::read_dir(root.join(kind.package_name())) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let kept = keep.current.as_deref() == Some(&name) || keep.previous.as_deref() == Some(&name);
        if !kept {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

pub fn status(root: &Path, kind: AdapterKind, runtime: Option<&AdapterRuntime>) -> AdapterStatus {
    let entry = pointers(root, kind);
    AdapterStatus {
        managed: entry.current.is_some(),
        adapter_version: runtime.and_then(|r| r.adapter_version.clone()),
        cli_version: runtime.and_then(|r| r.cli.as_ref()?.version.clone()),
        current: entry.current,
        previous: entry.previous,
        installed_versions: installed_versions(root, kind),
    }
}

/// The adapter that launches will use, with its bundled CLI: the managed
/// version when there is one, else whatever npm has installed globally.
pub async fn effective_runtime(root: &Path, kind: AdapterKind) -> Option<AdapterRuntime> {
    if let Some(dir) = active_package_dir(root, kind) {
        if let Some(runtime) = inspect_adapter(&dir, kind) {
            return Some(runtime);
        }
    }
    crate::acp::adapter_cli::installed_adapter(kind).await
}

/// Install `version` (unless present), probe it, and make it current.
///
/// `install` fills a directory with `<package>@<version>`; `probe` starts the
/// adapter found at the given package directory. Both are injected so the
/// sequencing can be tested without npm or node.
pub async fn upgrade_with<I, IFut, P, PFut>(
    root: &Path,
    kind: AdapterKind,
    version: &str,
    install: I,
    probe: P,
) -> Result<UpgradeOutcome, StoreError>
where
    I: FnOnce(PathBuf, String) -> IFut,
    IFut: Future<Output = Result<(), String>>,
    P: FnOnce(PathBuf) -> PFut,
    PFut: Future<Output = ProbeReport>,
{
    let before = pointers(root, kind);
    let final_dir = version_dir(root, kind, version);
    let freshly_installed = !is_installed(root, kind, version);
    if freshly_installed {
        let partial = final_dir.with_file_name(format!("{version}.partial"));
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).map_err(|e| StoreError::Io(e.to_string()))?;
        let spec = format!("{}@{version}", kind.npm_package());
        if let Err(detail) = install(partial.clone(), spec).await {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(StoreError::Install(detail));
        }
        let _ = std::fs::remove_dir_all(&final_dir);
        std::fs::rename(&partial, &final_dir).map_err(|e| StoreError::Io(e.to_string()))?;
        if !is_installed(root, kind, version) {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err(StoreError::Install(
                "npm finished but the adapter package is missing".into(),
            ));
        }
    }
    let report = probe(package_dir_of(root, kind, version)).await;
    if !report.ok {
        // Only remove what this call created; a version that was already
        // installed (possibly the one still in use) stays.
        if freshly_installed {
            let _ = std::fs::remove_dir_all(&final_dir);
        }
        return Err(StoreError::ProbeFailed {
            version: version.to_string(),
            kept: before.current.clone().unwrap_or_else(|| "the existing install".into()),
            detail: report.detail,
        });
    }
    activate(root, kind, version)?;
    prune(root, kind);
    Ok(UpgradeOutcome {
        version: version.to_string(),
        previous: pointers(root, kind).previous,
        probe: report,
    })
}

/// `npm install --prefix <dir> <spec>` with a time limit; Err carries npm's last words.
pub async fn npm_install(dir: PathBuf, spec: String) -> Result<(), String> {
    let npm = which::which("npm").map_err(|_| "npm is not installed or not on PATH".to_string())?;
    let mut command = crate::process::tokio_command(&npm);
    command
        .arg("install")
        .arg("--prefix")
        .arg(&dir)
        .args(["--include=optional", "--no-audit", "--no-fund", "--loglevel=error"])
        .arg(format!(
            "--registry={}",
            crate::commands::acp::NPM_OFFICIAL_REGISTRY
        ))
        .arg(spec)
        .kill_on_drop(true);
    let output = tokio::time::timeout(NPM_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            "npm install timed out; what was downloaded is cached, so trying again continues from there"
                .to_string()
        })?
        .map_err(|e| format!("cannot run npm: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let start = stderr.len().saturating_sub(NPM_ERROR_TAIL);
    let start = (start..=stderr.len())
        .find(|i| stderr.is_char_boundary(*i))
        .unwrap_or(stderr.len());
    Err(stderr[start..].trim().to_string())
}

static UPGRADE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Upgrade `kind` to `requested`, or to the newest published version.
pub async fn upgrade(
    root: &Path,
    kind: AdapterKind,
    requested: Option<&str>,
) -> Result<UpgradeOutcome, StoreError> {
    let version = match requested.map(str::trim).filter(|v| !v.is_empty()) {
        Some(raw) => crate::commands::acp::sanitize_custom_version(raw)
            .ok_or_else(|| StoreError::InvalidVersion(raw.to_string()))?,
        None => crate::acp::latest_release::fetch_npm_latest(&kind.npm_package())
            .await
            .map_err(|e| StoreError::Lookup(e.to_string()))?
            .ok_or_else(|| StoreError::Lookup("the registry has no such package".into()))?,
    };
    let _guard = UPGRADE_LOCK.lock().await;
    let node = which::which("node").map_err(|_| StoreError::Install("node is not installed".into()))?;
    upgrade_with(root, kind, &version, npm_install, |package_dir| async move {
        let scratch = std::env::temp_dir();
        match crate::acp::adapter_probe::adapter_command(&node, &package_dir) {
            Ok(command) => {
                crate::acp::adapter_probe::probe_command(
                    command,
                    &scratch,
                    crate::acp::adapter_probe::PROBE_TIMEOUT,
                )
                .await
            }
            Err(detail) => ProbeReport { ok: false, needs_login: false, detail },
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "adapter-store-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const KIND: AdapterKind = AdapterKind::Codex;

    /// What a successful `npm install` leaves behind.
    fn lay_down(dir: &Path, version: &str) {
        let package = adapter_package_dir(&dir.join("node_modules"), KIND);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.json"), format!(r#"{{"version":"{version}"}}"#)).unwrap();
        let bin = dir.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(KIND.package_name()), "").unwrap();
    }

    async fn ok_install(dir: PathBuf, spec: String) -> Result<(), String> {
        let version = spec.rsplit('@').next().unwrap().to_string();
        lay_down(&dir, &version);
        Ok(())
    }

    async fn passing(_: PathBuf) -> ProbeReport {
        ProbeReport { ok: true, needs_login: false, detail: "ok".into() }
    }

    async fn failing(_: PathBuf) -> ProbeReport {
        ProbeReport { ok: false, needs_login: false, detail: "initialize timed out".into() }
    }

    async fn install(root: &Path, version: &str) {
        upgrade_with(root, KIND, version, ok_install, passing).await.unwrap();
    }

    #[tokio::test]
    async fn a_first_install_becomes_current_and_gets_a_launch_command() {
        let root = temp("first");
        let outcome = upgrade_with(&root, KIND, "2.3.0", ok_install, passing).await.unwrap();
        assert_eq!(outcome.previous, None);
        assert_eq!(pointers(&root, KIND).current.as_deref(), Some("2.3.0"));
        assert!(managed_command(&root, KIND).unwrap().ends_with(".bin/codex-acp"));
        assert!(active_package_dir(&root, KIND).unwrap().join("package.json").is_file());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn upgrading_keeps_the_old_version_as_previous() {
        let root = temp("keep");
        install(&root, "2.2.2").await;
        let outcome = upgrade_with(&root, KIND, "2.3.0", ok_install, passing).await.unwrap();
        assert_eq!(outcome.previous.as_deref(), Some("2.2.2"));
        assert_eq!(installed_versions(&root, KIND), vec!["2.2.2", "2.3.0"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_failed_probe_removes_the_new_version_and_keeps_the_old_one() {
        let root = temp("probe-fail");
        install(&root, "2.2.2").await;
        let error = upgrade_with(&root, KIND, "2.3.0", ok_install, failing).await.unwrap_err();
        assert!(matches!(error, StoreError::ProbeFailed { .. }));
        assert!(error.to_string().contains("2.2.2 stays active"), "{error}");
        assert_eq!(pointers(&root, KIND).current.as_deref(), Some("2.2.2"));
        assert_eq!(installed_versions(&root, KIND), vec!["2.2.2"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_failed_install_leaves_no_trace() {
        let root = temp("install-fail");
        install(&root, "2.2.2").await;
        let error = upgrade_with(
            &root,
            KIND,
            "2.3.0",
            |_, _| async { Err::<(), _>("E404 not found".to_string()) },
            passing,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, StoreError::Install(_)));
        assert_eq!(installed_versions(&root, KIND), vec!["2.2.2"]);
        assert!(!root.join("codex-acp/2.3.0.partial").exists());
        assert_eq!(pointers(&root, KIND).current.as_deref(), Some("2.2.2"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_install_that_produced_nothing_is_rejected() {
        let root = temp("empty");
        let error = upgrade_with(&root, KIND, "2.3.0", |_, _| async { Ok(()) }, passing)
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::Install(_)), "{error}");
        assert!(installed_versions(&root, KIND).is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_failing_probe_never_deletes_a_version_that_was_already_there() {
        let root = temp("reprobe");
        install(&root, "2.2.2").await;
        let installs = Cell::new(0);
        let error = upgrade_with(
            &root,
            KIND,
            "2.2.2",
            |_, _| {
                installs.set(installs.get() + 1);
                async { Ok(()) }
            },
            failing,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, StoreError::ProbeFailed { .. }));
        assert_eq!(installs.get(), 0, "an installed version is not fetched again");
        assert_eq!(installed_versions(&root, KIND), vec!["2.2.2"]);
        assert_eq!(pointers(&root, KIND).current.as_deref(), Some("2.2.2"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn only_current_and_previous_survive_a_third_upgrade() {
        let root = temp("prune");
        install(&root, "2.1.0").await;
        install(&root, "2.2.2").await;
        install(&root, "2.3.0").await;
        assert_eq!(installed_versions(&root, KIND), vec!["2.2.2", "2.3.0"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn rollback_swaps_and_can_swap_back() {
        let root = temp("rollback");
        install(&root, "2.2.2").await;
        install(&root, "2.3.0").await;
        assert_eq!(rollback(&root, KIND).unwrap(), "2.2.2");
        let after = pointers(&root, KIND);
        assert_eq!((after.current.as_deref(), after.previous.as_deref()), (Some("2.2.2"), Some("2.3.0")));
        assert_eq!(rollback(&root, KIND).unwrap(), "2.3.0");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn rollback_without_a_previous_version_is_an_error() {
        let root = temp("no-previous");
        install(&root, "2.2.2").await;
        assert!(matches!(rollback(&root, KIND), Err(StoreError::NoPrevious)));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_pointer_to_a_deleted_directory_is_ignored() {
        let root = temp("dangling");
        install(&root, "2.2.2").await;
        install(&root, "2.3.0").await;
        std::fs::remove_dir_all(version_dir(&root, KIND, "2.3.0")).unwrap();
        let entry = pointers(&root, KIND);
        assert_eq!(entry.current, None);
        assert_eq!(entry.previous.as_deref(), Some("2.2.2"));
        assert!(managed_command(&root, KIND).is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_interrupted_install_is_not_listed_and_is_cleaned_up() {
        let root = temp("partial");
        install(&root, "2.2.2").await;
        std::fs::create_dir_all(root.join("codex-acp/2.3.0.partial/node_modules")).unwrap();
        assert_eq!(installed_versions(&root, KIND), vec!["2.2.2"]);
        prune(&root, KIND);
        assert!(!root.join("codex-acp/2.3.0.partial").exists());
        std::fs::remove_dir_all(root).ok();
    }

    /// Uses the real npm registry and starts the real adapters, so it is slow
    /// and needs network: `cargo test --lib real_upgrade_round_trip -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn real_upgrade_round_trip() {
        let root = temp("real");
        let first = upgrade(&root, KIND, Some("2.2.2")).await.unwrap();
        assert_eq!(first.version, "2.2.2");
        let second = upgrade(&root, KIND, None).await.unwrap();
        assert_eq!(second.previous.as_deref(), Some("2.2.2"));
        let runtime = effective_runtime(&root, KIND).await.unwrap();
        println!(
            "latest {} bundles CLI {:?}; probe: {:?}",
            second.version,
            runtime.cli.as_ref().and_then(|c| c.version.clone()),
            second.probe
        );
        assert_eq!(runtime.adapter_version.as_deref(), Some(second.version.as_str()));
        assert!(runtime.cli.is_some());

        // A version that does not exist is refused and changes nothing.
        let missing = upgrade(&root, KIND, Some("99.0.0")).await.unwrap_err();
        assert!(matches!(missing, StoreError::Install(_)), "{missing}");
        assert_eq!(pointers(&root, KIND).current.as_deref(), Some(second.version.as_str()));

        assert_eq!(rollback(&root, KIND).unwrap(), "2.2.2");
        assert!(managed_command(&root, KIND).is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn nothing_managed_means_no_command() {
        let root = temp("none");
        assert!(managed_command(&root, KIND).is_none());
        assert_eq!(pointers(&root, KIND), Pointers::default());
        std::fs::remove_dir_all(root).ok();
    }
}
