use super::*;
use serde_json::json;

fn member() -> HallMember {
    HallMember {
        member_id: uuid::Uuid::new_v4().to_string(),
        name: "reviewer".into(),
        role: "review".into(),
        cwd: "/members/reviewer".into(),
        project_id: None,
        engine: "codex".into(),
        assistant_uid: Some("preset".into()),
        assistant_revision: Some(7),
        enabled: false,
        revision: 1,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}
fn declaration(model: Option<&str>, write: &[&str]) -> Declaration {
    serde_json::from_value(json!({"model":model,"scope":{"read":["**"],"write":write,"shell":[],"network":"none"},"skills":{"shared":[],"own":[]}})).unwrap()
}
fn source() -> Template {
    Template { name: "preset".into(), agent_type: "codex".into(), enabled: true, revision: 7, deleted_at: None,
        agent_options: r#"{"config_values":{"model":"preset-model"}}"#.into(),
        label_snapshot: r#"{"agent":"Codex","options":[{"id":"model","kind":"select","name":"Model","value_label":"Preset"}]}"#.into(),
        instructions: "private fixture instructions".into() }
}

#[test]
fn a_candidate_owns_its_source_and_its_hash_tracks_instruction_changes() {
    let member = member();
    let mut row = source();
    let (frozen, issues) = build(
        member.clone(),
        "c".repeat(64),
        Some(row.clone()),
        declaration(None, &[]),
        options(&member, Some(&row)).unwrap(),
    );
    assert!(issues.is_empty());
    let frozen = frozen.unwrap();
    let first = frozen.summary().unwrap().snapshot_version;
    row.instructions.push_str(" changed");
    assert_eq!(frozen.summary().unwrap().snapshot_version, first);
    let (later, _) = build(
        member.clone(),
        "c".repeat(64),
        Some(row.clone()),
        declaration(None, &[]),
        options(&member, Some(&row)).unwrap(),
    );
    assert_ne!(later.unwrap().summary().unwrap().snapshot_version, first);
}

#[test]
fn a_declared_model_overrides_the_template_and_an_empty_declaration_inherits_it() {
    let member = member();
    let row = source();
    let (frozen, _) = build(
        member.clone(),
        "c".repeat(64),
        Some(row.clone()),
        declaration(Some("member-model"), &[]),
        options(&member, Some(&row)).unwrap(),
    );
    let frozen = frozen.unwrap();
    assert_eq!(
        frozen.data.desired_options.config_values["model"],
        "member-model"
    );
    assert_eq!(frozen.summary().unwrap().model_source, "member");
    let (inherited, _) = build(
        member.clone(),
        "c".repeat(64),
        Some(row.clone()),
        declaration(None, &[]),
        options(&member, Some(&row)).unwrap(),
    );
    let inherited = inherited.unwrap();
    assert_eq!(
        inherited.data.desired_options.config_values["model"],
        "preset-model"
    );
    assert_eq!(inherited.summary().unwrap().model_source, "template");
    assert_eq!(inherited.data.permission_policy, PermissionPolicy::Wait);
}

#[test]
fn unsafe_conflicting_or_overbroad_modes_never_produce_a_candidate() {
    for (mode, config_mode, expected) in [
        ("agent-full-access", None, "unsafe_mode"),
        ("workspace-write", None, "mode_exceeds_scope"),
        ("read-only", Some("workspace-write"), "conflicting_mode"),
    ] {
        let mut desired = AgentDelegationDefaults {
            mode_id: Some(mode.into()),
            ..Default::default()
        };
        if let Some(value) = config_mode {
            desired.config_values.insert("mode".into(), value.into());
        }
        let (frozen, issues) = build(
            member(),
            "c".repeat(64),
            None,
            declaration(None, &[]),
            desired,
        );
        assert!(frozen.is_none());
        assert!(issues.iter().any(|issue| issue.code == expected));
    }
    let (frozen, _) = build(
        member(),
        "c".repeat(64),
        None,
        declaration(None, &["reports/**"]),
        AgentDelegationDefaults::default(),
    );
    assert_eq!(
        frozen.unwrap().summary().unwrap().requested_mode,
        "workspace-write"
    );
    let desired = AgentDelegationDefaults {
        mode_id: Some("read-only".into()),
        ..Default::default()
    };
    let (frozen, _) = build(
        member(),
        "c".repeat(64),
        None,
        declaration(None, &["reports/**"]),
        desired,
    );
    assert_eq!(
        frozen.unwrap().summary().unwrap().requested_mode,
        "read-only"
    );
}

#[test]
fn oversized_template_content_is_blocked_without_creating_a_partial_candidate() {
    let mut row = source();
    row.instructions = "x".repeat(MAX_INSTRUCTIONS_CHARS + 1);
    let (frozen, issues) = build(
        member(),
        "c".repeat(64),
        Some(row),
        declaration(None, &[]),
        AgentDelegationDefaults::default(),
    );
    assert!(frozen.is_none());
    assert!(issues
        .iter()
        .any(|issue| issue.code == "template_content_invalid"));
}

#[test]
fn stored_candidates_round_trip_and_rebuild_scope_without_any_source_reads() {
    let member = member();
    let row = source();
    let (frozen, _) = build(
        member.clone(),
        "c".repeat(64),
        Some(row.clone()),
        declaration(Some("member-model"), &["reports/**"]),
        options(&member, Some(&row)).unwrap(),
    );
    let frozen = frozen.unwrap();
    let raw = frozen.encoded().unwrap();
    let restored = FrozenCandidate::restore(&raw).unwrap();
    assert_eq!(
        restored.summary().unwrap().snapshot_version,
        frozen.summary().unwrap().snapshot_version
    );
    assert!(restored
        .compiled_scope
        .matches_declared_write_path("reports/a.md"));
    assert!(!restored
        .compiled_scope
        .matches_declared_write_path("elsewhere/a.md"));
    assert_eq!(
        restored.data.template.unwrap().instructions,
        "private fixture instructions"
    );
    for (field, value) in [
        ("schema_version", json!(2)),
        ("permission_policy", json!("allow")),
        ("desired_options", json!({"mode_id":"agent-full-access"})),
    ] {
        let mut changed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        changed[field] = value;
        assert!(FrozenCandidate::restore(&changed.to_string()).is_err());
    }
    let mut unsupported: serde_json::Value = serde_json::from_str(&raw).unwrap();
    unsupported["member"]["future_field"] = json!("not supported by this schema");
    assert!(FrozenCandidate::restore(&unsupported.to_string()).is_err());
}

#[test]
fn unsupported_native_scopes_never_produce_a_codex_candidate() {
    for rule in ["notes/*.md", "*.md", "reports", ".onedesk/private/**"] {
        for write in [false, true] {
            let mut declared = declaration(None, &[]);
            if write {
                declared.scope.write = vec![rule.into()];
            } else {
                declared.scope.read = vec![rule.into()];
            }
            let (frozen, issues) = build(
                member(),
                "c".repeat(64),
                None,
                declared,
                AgentDelegationDefaults::default(),
            );
            assert!(frozen.is_none(), "{rule}");
            assert!(issues.iter().any(|issue| issue.code == "invalid_path_rule"));
        }
    }
}

#[test]
fn legacy_v1_archive_keeps_its_exact_bytes_and_fingerprint() {
    // Independent pre-key format fixture: field order and omissions are part
    // of the stored v1 fingerprint, so this must never be rebuilt dynamically.
    const RAW: &str = r#"{"schema_version":1,"member":{"memberId":"00000000-0000-4000-8000-000000000001","name":"legacy reviewer","role":"review","cwd":"/members/legacy","projectId":null,"engine":"codex","assistantUid":null,"assistantRevision":null,"enabled":false,"revision":1,"createdAt":"2026-10-27T00:00:00Z","updatedAt":"2026-10-27T00:00:00Z"},"check_version":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","template":null,"declaration":{"model":null,"scope":{"read":["**"],"write":[],"shell":[],"network":"none"},"skills":{"shared":[],"own":[]}},"desired_options":{"mode_id":"read-only"},"permission_policy":"wait"}"#;
    let restored = FrozenCandidate::restore(RAW).unwrap();
    assert_eq!(restored.encoded().unwrap(), RAW);
    assert_eq!(
        restored.summary().unwrap().snapshot_version,
        "ca4319fc5303b78dbeb945cf4ad044a1b56e64985e0841b6fc997bd9d5059d36"
    );
    assert_eq!(
        restored
            .launch_profile(uuid::Uuid::new_v4())
            .access
            .onedesk_key,
        "none"
    );
}

#[test]
fn legacy_narrow_glob_archive_is_verified_without_widening_or_authorizing_launch() {
    const RAW: &str = r#"{"schema_version":1,"member":{"memberId":"00000000-0000-4000-8000-000000000001","name":"legacy reviewer","role":"review","cwd":"/members/legacy","projectId":null,"engine":"codex","assistantUid":null,"assistantRevision":null,"enabled":false,"revision":1,"createdAt":"2026-10-27T00:00:00Z","updatedAt":"2026-10-27T00:00:00Z"},"check_version":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","template":null,"declaration":{"model":null,"scope":{"read":["notes/*.md"],"write":[],"shell":[],"network":"none"},"skills":{"shared":[],"own":[]}},"desired_options":{"mode_id":"read-only"},"permission_policy":"wait"}"#;
    let restored = FrozenCandidate::restore(RAW).unwrap();
    assert_eq!(restored.encoded().unwrap(), RAW);
    assert_eq!(
        restored.summary().unwrap().snapshot_version,
        "3e17d45e134ad167968a99931e70c6c5fa4e664bfe36f4fa13dc7ccfb8bafdf5"
    );
    assert!(restored
        .compiled_scope
        .matches_declared_read_path("notes/a.md"));
    assert!(!restored
        .compiled_scope
        .matches_declared_read_path("notes/sub/a.md"));
    let profile = restored.launch_profile(uuid::Uuid::new_v4());
    assert!(crate::hall::members::launch::scope_directory(&profile.access.read[0]).is_err());
}
