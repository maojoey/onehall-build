use super::*;

fn member(path: &Path) -> HallMember {
    HallMember {
        member_id: uuid::Uuid::new_v4().to_string(),
        name: "writer".into(),
        role: "review".into(),
        cwd: path.canonicalize().unwrap().to_str().unwrap().into(),
        project_id: None,
        engine: "codex".into(),
        assistant_uid: None,
        assistant_revision: None,
        enabled: false,
        revision: 1,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}
fn version(member: &HallMember) -> String {
    combine(
        member,
        Some(read::read_all(Path::new(&member.cwd)).unwrap()),
    )
    .file_version
    .unwrap()
}
fn no_temporaries(path: &Path) {
    assert!(std::fs::read_dir(path).unwrap().all(|file| !file
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
}

#[test]
fn an_external_edit_during_staging_is_rejected_and_its_bytes_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let member = member(dir.path());
    let manual = dir.path().join("agent.yaml");
    let observed = version(&member);
    let result = replace_checked(
        &member,
        &observed,
        b"schema_version: 1\nmodel: draft\n",
        || {
            std::fs::write(&manual, "# external\nschema_version: 1\nmodel: external\n").unwrap();
        },
    );
    assert!(result.is_err());
    assert!(std::fs::read_to_string(manual)
        .unwrap()
        .contains("external"));
    assert!(!dir.path().join(".onehall/agent.ui.yaml").exists());
    no_temporaries(&dir.path().join(".onehall"));
}

#[test]
fn a_cooperative_save_lock_refuses_another_writer() {
    let dir = tempfile::tempdir().unwrap();
    let member = member(dir.path());
    let root = read::Root::open(Path::new(&member.cwd)).unwrap();
    let _lock = platform::Writer::open(&root, Path::new(&member.cwd)).unwrap();
    assert!(replace(&member, &version(&member), b"schema_version: 1\n").is_err());
    assert!(!dir.path().join(".onehall/agent.ui.yaml").exists());
}

#[test]
fn failed_atomic_replacement_preserves_a_replacement_directory_and_cleans_its_temp() {
    let dir = tempfile::tempdir().unwrap();
    let member = member(dir.path());
    let target = dir.path().join(".onehall/agent.ui.yaml");
    let root = read::Root::open(Path::new(&member.cwd)).unwrap();
    let mut writer = platform::Writer::open(&root, Path::new(&member.cwd)).unwrap();
    writer.stage(b"schema_version: 1\n").unwrap();
    std::fs::create_dir(&target).unwrap();
    assert!(writer.commit().is_err());
    drop(writer);
    assert!(target.is_dir());
    no_temporaries(&dir.path().join(".onehall"));
}

#[cfg(unix)]
#[test]
fn moving_the_overlay_directory_cannot_redirect_the_write() {
    let dir = tempfile::tempdir().unwrap();
    let member = member(dir.path());
    let result = replace_checked(
        &member,
        &version(&member),
        b"schema_version: 1\nmodel: draft\n",
        || {
            std::fs::rename(dir.path().join(".onehall"), dir.path().join("moved")).unwrap();
            std::fs::create_dir(dir.path().join(".onehall")).unwrap();
        },
    );
    assert!(result.is_err());
    assert!(!dir.path().join(".onehall/agent.ui.yaml").exists());
    assert!(!dir.path().join("moved/agent.ui.yaml").exists());
    no_temporaries(&dir.path().join("moved"));
}

#[cfg(unix)]
#[test]
fn replacing_the_overlay_with_a_symlink_during_staging_preserves_the_outside_file() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("outside.yaml");
    std::fs::write(&target, "private fixture").unwrap();
    let member = member(dir.path());
    let result = replace_checked(&member, &version(&member), b"schema_version: 1\n", || {
        std::os::unix::fs::symlink(&target, dir.path().join(".onehall/agent.ui.yaml")).unwrap();
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "private fixture");
    no_temporaries(&dir.path().join(".onehall"));
}

#[cfg(unix)]
#[test]
fn a_lock_symlink_is_never_opened_or_changed() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    let member = member(dir.path());
    std::fs::create_dir(dir.path().join(".onehall")).unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join(".onehall/.agent.ui.lock")).unwrap();
    assert!(replace(&member, &version(&member), b"schema_version: 1\n").is_err());
    assert_eq!(std::fs::read(outside.path()).unwrap(), b"");
    assert!(!dir.path().join(".onehall/agent.ui.yaml").exists());
}
