//! Pure policy tests. A simulated platform proves the decision path only; it is
//! never evidence that the native sandbox or exit domain works on that system.
use super::*;
use crate::hall::members::MemberAccess;
use serde_json::json;
use std::collections::BTreeMap;

fn profile(engine: &str, mode: Option<ExecutionMode>) -> LaunchProfile {
    LaunchProfile {
        member_id: "fixture".into(),
        engine: engine.into(),
        member_revision: 1,
        cwd: "/fixture".into(),
        assistant_uid: None,
        mode: "read-only".into(),
        config: BTreeMap::new(),
        brief: String::new(),
        access: MemberAccess {
            read: vec!["**".into()],
            write: vec![],
            network: "none".into(),
            onedesk_key: "none".into(),
        },
        execution_mode: mode,
        capabilities: mode.map(|m| capabilities(engine, Some(m), Platform::Macos)),
    }
}

#[test]
fn modes_and_capabilities_use_the_published_json() {
    assert_eq!(
        serde_json::to_value(ExecutionMode::LocalAssistant).unwrap(),
        json!("local_assistant")
    );
    for raw in ["bypass", "auto", "Restricted", ""] {
        assert!(serde_json::from_value::<ExecutionMode>(json!(raw)).is_err());
    }
    let caps = capabilities("codex", Some(ExecutionMode::Restricted), Platform::Linux);
    assert_eq!(
        serde_json::to_value(&caps).unwrap(),
        json!({
            "readBoundary": "unknown", "writeBoundary": "unknown",
            "networkBoundary": "unknown", "permissionRequests": "supported",
            "hallIdentity": true, "hallReport": true,
            "exitCoverage": "linux_cgroup_v2",
            "engineVersion": "codex-cli 0.163.0",
            "adapterVersion": "codex-acp 2.2.2", "evidenceId": null
        })
    );
    let mut forged = serde_json::to_value(&caps).unwrap();
    forged["sandbox"] = json!("verified");
    assert!(serde_json::from_value::<ExecutionCapabilities>(forged).is_err());
}

#[test]
fn legacy_members_keep_their_history_and_claude_is_never_marked_restricted() {
    assert_eq!(
        effective_mode("codex", None),
        Some(ExecutionMode::Restricted)
    );
    assert_eq!(effective_mode("claude_code", None), None);
    let legacy = capabilities("claude_code", None, Platform::Linux);
    assert_eq!(legacy.read_boundary, Enforcement::Unknown);
    assert_eq!(legacy.permission_requests, PermissionRequests::Unknown);
    assert_eq!(legacy.engine_version, None);
    let chosen = capabilities(
        "claude_code",
        Some(ExecutionMode::LocalAssistant),
        Platform::Macos,
    );
    assert_eq!(
        chosen.exit_coverage,
        ExitCoverage::MacosProcessGroupBestEffort
    );
    assert_eq!(chosen.write_boundary, Enforcement::NotEnforced);
}

#[test]
fn only_matching_recorded_evidence_can_verify_a_boundary() {
    let record = |engine_version: &'static str| Evidence {
        id: "fixture-evidence",
        engine: "codex",
        platform: Platform::Linux,
        engine_version,
        adapter_version: CODEX_ADAPTER_VERSION,
        read: Enforcement::Verified,
        write: Enforcement::Verified,
        network: Enforcement::Unknown,
    };
    let restricted = Some(ExecutionMode::Restricted);
    let hit = capabilities_with(
        "codex",
        restricted,
        Platform::Linux,
        &[record(CODEX_ENGINE_VERSION)],
    );
    assert_eq!(hit.read_boundary, Enforcement::Verified);
    assert_eq!(hit.evidence_id.as_deref(), Some("fixture-evidence"));
    // An upgraded engine, another platform or another mode invalidates it.
    for (platform, mode, evidence) in [
        (Platform::Linux, restricted, record("codex-cli 0.164.0")),
        (Platform::Macos, restricted, record(CODEX_ENGINE_VERSION)),
        (
            Platform::Linux,
            Some(ExecutionMode::LocalAssistant),
            record(CODEX_ENGINE_VERSION),
        ),
    ] {
        let miss = capabilities_with("codex", mode, platform, &[evidence]);
        assert_ne!(miss.read_boundary, Enforcement::Verified);
        assert_eq!(miss.evidence_id, None);
    }
    // No recorded evidence exists yet, so nothing may report verified.
    assert!(EVIDENCE.is_empty());
}

#[test]
fn forged_stored_claims_are_inconsistent() {
    let good = capabilities("codex", Some(ExecutionMode::Restricted), Platform::Linux);
    assert!(consistent("codex", ExecutionMode::Restricted, &good));
    let local = capabilities(
        "codex",
        Some(ExecutionMode::LocalAssistant),
        Platform::Linux,
    );
    assert!(consistent("codex", ExecutionMode::LocalAssistant, &local));
    let tweak = |base: &ExecutionCapabilities, f: fn(&mut ExecutionCapabilities)| {
        let mut c = base.clone();
        f(&mut c);
        c
    };
    for forged in [
        tweak(&good, |c| c.read_boundary = Enforcement::Verified),
        tweak(&good, |c| c.evidence_id = Some("invented".into())),
        tweak(&good, |c| c.write_boundary = Enforcement::NotEnforced),
        tweak(&good, |c| c.hall_identity = false),
        tweak(&good, |c| c.engine_version = None),
        tweak(&good, |c| c.adapter_version = Some("codex-acp 9".into())),
    ] {
        assert!(!consistent("codex", ExecutionMode::Restricted, &forged));
    }
    for forged in [
        tweak(&local, |c| c.network_boundary = Enforcement::Verified),
        tweak(&local, |c| c.evidence_id = Some("invented".into())),
        tweak(&local, |c| {
            c.engine_version = Some("codex-cli 0.163.0".into())
        }),
    ] {
        assert!(!consistent("codex", ExecutionMode::LocalAssistant, &forged));
    }
}

#[test]
fn windows_restricted_codex_is_refused_but_other_paths_are_not() {
    let restricted = profile("codex", Some(ExecutionMode::Restricted));
    let windows = decide(&restricted, Platform::Windows);
    assert!(windows.is_err());
    // A legacy Codex member is restricted by history and is refused alike.
    assert!(decide(&profile("codex", None), Platform::Windows).is_err());
    let unix = decide(&restricted, Platform::Macos).unwrap();
    assert!(unix.bridge && unix.key);
    assert!(decide(
        &profile("claude_code", Some(ExecutionMode::Restricted)),
        Platform::Linux
    )
    .is_err());
}

#[test]
fn local_assistant_skips_the_bridge_and_never_gets_the_key() {
    for engine in ["codex", "claude_code"] {
        let mut p = profile(engine, Some(ExecutionMode::LocalAssistant));
        p.capabilities = Some(capabilities(engine, p.execution_mode, Platform::Windows));
        let d = decide(&p, Platform::Windows).unwrap();
        assert_eq!(d.mode, Some(ExecutionMode::LocalAssistant));
        assert!(!d.bridge && !d.key, "{engine}");
        assert_eq!(marker(&d), "local_assistant");
        p.access.onedesk_key = "read".into();
        assert!(decide(&p, Platform::Windows).is_err());
    }
    let legacy = decide(&profile("claude_code", None), Platform::Windows).unwrap();
    assert!(!legacy.bridge && legacy.key);
    assert_eq!(marker(&legacy), "legacy");
}

#[test]
fn changed_capabilities_require_a_new_snapshot() {
    let mut p = profile("codex", Some(ExecutionMode::Restricted));
    assert!(decide(&p, Platform::Macos).is_ok());
    // Frozen on one platform, launched on another: the exit coverage differs.
    assert!(decide(&p, Platform::Linux).is_err());
    p.capabilities.as_mut().unwrap().engine_version = Some("codex-cli 0.164.0".into());
    assert!(decide(&p, Platform::Macos).is_err());
    // Capabilities cannot exist without a chosen mode.
    let mut orphan = profile("claude_code", None);
    orphan.capabilities = Some(capabilities("claude_code", None, Platform::Macos));
    assert!(decide(&orphan, Platform::Macos).is_err());
}

#[test]
fn the_manager_requires_the_bridge_unless_a_local_assistant_was_chosen() {
    assert!(bridge_required(None));
    assert!(bridge_required(Some("restricted")));
    assert!(bridge_required(Some("legacy")));
    assert!(bridge_required(Some("anything-else")));
    assert!(!bridge_required(Some("local_assistant")));
}
